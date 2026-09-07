"use strict";

// The navigation loop lives entirely on this side. A keypress must not wait on
// the command channel, on disk, or on a JPEG decoder: by the time the user
// presses H, the next photo is already decoded and sitting in the DOM, and the
// key does nothing but change which element is visible.

const { invoke, convertFileSrc } =
  window.__TAURI__?.core ?? window.__TAURI_INTERNALS__ ?? {};

const AHEAD = 5;
const BEHIND = 3;
const RING = AHEAD + BEHIND + 1;

/// A R S T G sit under the home row in Colemak-DH. `event.key` is what makes
/// that work: `event.code` reports the physical QWERTY position, which differs
/// depending on whether the layout lives in the OS or in the keyboard firmware.
const STARS = { a: 1, r: 2, s: 3, t: 4, g: 5 };

/// `xmp:Label` stores the colour under its standard English name, which is
/// what every other program expects to read. The chip shows it in Portuguese.
const LABEL_NAMES = { Green: "verde" };

const el = (id) => document.getElementById(id);
const dom = {
  folder: el("folder"),
  counter: el("counter"),
  stage: el("stage"),
  ring: el("ring"),
  empty: el("empty"),
  emptyMsg: el("empty-msg"),
  rating: el("rating"),
  label: el("label"),
  reject: el("reject"),
  collection: el("collection"),
  name: el("name"),
  latency: el("latency"),
  newCollection: el("newCollection"),
  collectionName: el("collection-name"),
  collectionError: el("collection-error"),
  move: el("move"),
  moveList: el("move-list"),
  apply: el("apply"),
  applyBody: el("apply-body"),
  applyBlockers: el("apply-blockers"),
  applyBlockerList: el("apply-blocker-list"),
  applyGo: el("apply-go"),
  report: el("report"),
  reportTitle: el("report-title"),
  reportBody: el("report-body"),
  settings: el("settings"),
  help: el("help"),
};

const state = {
  photos: [],
  marks: [],
  collections: [],
  assigned: [],
  index: 0,
  modal: null,
  plan: null,
  slots: [],
};

// ---------------------------------------------------------------- ring

for (let i = 0; i < RING; i++) {
  const img = document.createElement("img");
  img.draggable = false;
  dom.ring.appendChild(img);
  state.slots.push({ el: img, index: null, ready: false });
}

function frameUrl(index) {
  return convertFileSrc(String(index), "zaru");
}

/// Places `index` in the ring and starts decoding it. Returns its slot.
///
/// The victim is the slot holding whatever the user is furthest from, so the
/// frames just behind survive a change of direction.
function assignSlot(index) {
  const held = state.slots.find((s) => s.index === index);
  if (held) return held;

  let victim = state.slots.find((s) => s.index === null);
  if (!victim) {
    let worst = -1;
    for (const slot of state.slots) {
      const distance = Math.abs(slot.index - state.index);
      if (distance > worst) {
        worst = distance;
        victim = slot;
      }
    }
  }

  const photo = state.photos[index];
  victim.index = index;
  victim.ready = false;
  fit(victim, photo);
  victim.el.src = frameUrl(index);
  // Decoding ahead of time is the whole point: without it the keypress pays
  // for a full-resolution JPEG decode and the loop stutters.
  victim.el.decode().then(
    () => { victim.ready = true; },
    () => {},
  );
  return victim;
}

/// The stage never changes size. A rotated frame is sized against the swapped
/// axes so it still lands inside the same box.
function fit(slot, photo) {
  const rotated = photo.rotation === 90 || photo.rotation === 270;
  const w = dom.stage.clientWidth;
  const h = dom.stage.clientHeight;
  slot.el.style.width = `${rotated ? h : w}px`;
  slot.el.style.height = `${rotated ? w : h}px`;
  slot.el.style.transform =
    `translate(-50%, -50%) rotate(${photo.rotation}deg)` +
    (photo.mirrored ? " scaleX(-1)" : "");
}

function windowOrder(index) {
  const out = [];
  for (let d = 0; d <= AHEAD; d++) {
    if (index + d < state.photos.length) out.push(index + d);
  }
  for (let d = 1; d <= BEHIND; d++) {
    if (index - d >= 0) out.push(index - d);
  }
  return out;
}

let slidePending = null;
function fillRing() {
  for (const i of windowOrder(state.index)) assignSlot(i);
  // Tell the Rust pool where the window is, at most once per frame.
  if (slidePending === null) {
    slidePending = requestAnimationFrame(() => {
      slidePending = null;
      invoke("set_index", { index: state.index }).catch(() => {});
    });
  }
}

// ------------------------------------------------------------ rendering

function show() {
  if (!state.photos.length) return;
  const slot = assignSlot(state.index);
  for (const s of state.slots) s.el.classList.toggle("shown", s === slot);
  renderStatus();
}

/// A collection carries no meaning Zaru could colour it by, so the colour is a
/// hash of the name: stable between sessions, and clear of the three chips that
/// do mean something.
///
/// The hash is avalanched before it becomes a hue. A plain multiply-and-add
/// keeps neighbouring inputs neighbouring, which put "porsche" and "ferrari"
/// seven degrees apart — the same pink twice over. Lightness takes a second bit
/// so two names that still land close in hue separate anyway.
function chipColour(name) {
  let h = 0x811c9dc5;
  for (const ch of name) h = Math.imul(h ^ ch.codePointAt(0), 0x01000193) >>> 0;
  h ^= h >>> 16;
  h = Math.imul(h, 0x85ebca6b) >>> 0;
  h ^= h >>> 13;
  h = Math.imul(h, 0xc2b2ae35) >>> 0;
  h = (h ^ (h >>> 16)) >>> 0;
  // Both levels keep black text comfortably readable.
  return `hsl(${h % 360} 62% ${(h >>> 8) & 1 ? 82 : 71}%)`;
}

function renderStatus() {
  const photo = state.photos[state.index];
  const mark = state.marks[state.index];
  if (!photo) return;

  dom.counter.textContent = `${state.index + 1} / ${state.photos.length}`;
  dom.name.textContent = photo.name;

  const rejected = mark.rating === -1;
  const stars = rejected ? 0 : mark.rating;
  // Rejection and rating are one field, so the meter gives way to the chip
  // rather than sitting next to it claiming zero stars.
  dom.rating.hidden = rejected;
  dom.rating.querySelectorAll("i").forEach((cell, i) => {
    cell.classList.toggle("on", i < stars);
  });
  dom.reject.hidden = !rejected;
  dom.label.hidden = !mark.label;
  if (mark.label) {
    dom.label.textContent = LABEL_NAMES[mark.label] ?? mark.label.toLowerCase();
  }

  const collection = state.assigned[state.index];
  dom.collection.hidden = collection === null || collection === undefined;
  if (!dom.collection.hidden) {
    const name = state.collections[collection];
    dom.collection.textContent = name;
    dom.collection.style.setProperty("--chip", chipColour(name));
  }
}

function goto(index, pressedAt) {
  const next = Math.max(0, Math.min(state.photos.length - 1, index));
  // At either end the key does nothing, and timing a frame that never changed
  // would quietly flatter the measurement.
  if (next === state.index) return;
  if (pressedAt !== undefined) measureFrom(pressedAt);
  state.index = next;
  show();
  fillRing();
}

// ------------------------------------------------------- instrumentation

// Phase 1's acceptance is a number, not an impression: hold H down and read
// the time between the key and the paint that answers it.
const samples = [];
let keyAt = null;

function measureFrom(t) {
  keyAt = t;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      if (keyAt === null) return;
      samples.push(performance.now() - keyAt);
      keyAt = null;
      if (samples.length > 200) samples.shift();
      renderLatency();
    }),
  );
}

function renderLatency() {
  if (samples.length < 5) return;
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * q))];
  dom.latency.textContent = `mediana ${at(0.5).toFixed(1)} ms · p95 ${at(0.95).toFixed(1)} ms`;
}

// -------------------------------------------------------------- commands

/// Records a change and redraws. Never navigates: rejection advances the frame
/// itself, immediately, and must not be dragged back when the command answers.
async function applyChange(promise, confirmOn) {
  const change = await promise;
  if (!change) return;
  state.marks[change.index] = change.mark;
  state.assigned[change.index] = change.collection;
  if (change.index !== state.index) return;
  renderStatus();
  flash(confirmOn);
}

/// Answers "did that register?" without costing time. One pass, under 120ms.
function flash(node) {
  if (!node || node.hidden) return;
  node.classList.remove("flash");
  void node.offsetWidth;
  node.classList.add("flash");
  node.addEventListener("animationend", () => node.classList.remove("flash"), {
    once: true,
  });
}

/// Undo and redo have to bring the photo they repaired back into view, or the
/// user cannot see what changed.
async function applyJump(promise) {
  const change = await promise;
  if (!change) return;
  state.marks[change.index] = change.mark;
  state.assigned[change.index] = change.collection;
  if (change.index === state.index) {
    renderStatus();
    return;
  }
  state.index = change.index;
  show();
  fillRing();
}

async function openFolder(path) {
  const session = await invoke("open_folder", { path });
  state.photos = session.photos;
  state.marks = session.marks;
  state.collections = session.collections;
  state.assigned = session.assigned;
  state.index = 0;
  for (const slot of state.slots) {
    slot.index = null;
    slot.ready = false;
    slot.el.removeAttribute("src");
    slot.el.classList.remove("shown");
  }
  dom.folder.textContent = session.folder;
  dom.empty.hidden = true;
  document.body.classList.remove("idle");
  samples.length = 0;
  show();
  fillRing();
}

async function pickFolder() {
  const path = await invoke("pick_folder");
  if (!path) return;
  try {
    await openFolder(path);
  } catch (e) {
    dom.empty.hidden = false;
    dom.emptyMsg.textContent = String(e);
  }
}

// ----------------------------------------------------------- collections

/// `N` opens a text field, and while it is open every key belongs to it.
/// Without that the user drops into the mode by accident and the next five
/// keystrokes become a folder name.
function startNewCollection() {
  dom.collectionName.value = "";
  dom.collectionError.hidden = true;
  openModal("newCollection");
  dom.collectionName.focus();
}

async function confirmNewCollection() {
  try {
    state.collections = await invoke("new_collection", {
      name: dom.collectionName.value,
    });
    closeModal();
    // Straight into the picker: creating a collection is nearly always the
    // first half of putting this photo in it.
    startMove();
  } catch (e) {
    dom.collectionError.textContent = String(e);
    dom.collectionError.hidden = false;
    dom.collectionName.focus();
  }
}

/// `M` is a two-keystroke mode: the list opens, a digit picks, and it closes.
function startMove() {
  dom.moveList.replaceChildren();

  if (!state.collections.length) {
    const li = document.createElement("li");
    const p = document.createElement("span");
    p.className = "none";
    p.textContent = "Nenhuma coleção ainda. Aperte N para criar a primeira.";
    li.append(p);
    dom.moveList.append(li);
  } else {
    const counts = state.collections.map(
      (_, c) => state.assigned.filter((a) => a === c).length,
    );
    state.collections.forEach((name, c) => {
      const li = document.createElement("li");

      const key = document.createElement("kbd");
      key.textContent = String(c + 1);

      const label = document.createElement("span");
      const chip = document.createElement("span");
      chip.className = "chip chip-collection";
      chip.textContent = name;
      chip.style.setProperty("--chip", chipColour(name));
      label.append(chip);

      const count = document.createElement("b");
      count.textContent = counts[c] === 1 ? "1 foto" : `${counts[c]} fotos`;
      if (!counts[c]) count.textContent = "";

      li.append(key, label, count);
      dom.moveList.append(li);
    });
  }
  openModal("move");
}

function chooseCollection(collection) {
  closeModal();
  applyChange(invoke("assign", { index: state.index, collection }), dom.collection);
}

// ---------------------------------------------------------------- apply

/// Shows what Apply would do before it does any of it. Moving files is the one
/// irreversible thing in the app, and it should never be a surprise.
async function openApply() {
  const plan = await invoke("plan");
  state.plan = plan;

  dom.applyBody.replaceChildren();
  const row = (count, what, note) => {
    const dt = document.createElement("dt");
    dt.textContent = String(count);
    const dd = document.createElement("dd");
    dd.textContent = what;
    if (note) {
      const small = document.createElement("span");
      small.className = "note";
      small.textContent = ` ${note}`;
      dd.append(small);
    }
    dom.applyBody.append(dt, dd);
  };

  row(plan.evaluated, "fotos avaliadas");
  row(plan.sidecars, "vão receber .xmp");
  for (const move of plan.moves) {
    // The sidecar and the RAW+JPEG twin travel with the photo, so the file
    // count is usually higher than the photo count.
    const note = move.files !== move.photos ? `(${move.files} arquivos)` : "";
    row(move.photos, `vão para ${move.collection}/`, note);
  }
  // Rejected is a note in the metadata and nothing more. Zaru never deletes.
  row(plan.rejected, "rejeitadas — permanecem onde estão");
  row(plan.untouched, "sem marcação, ficam como estão");

  const blocked = plan.blockers.length > 0;
  dom.applyBlockers.hidden = !blocked;
  dom.applyBlockerList.replaceChildren(
    ...plan.blockers.map((b) => {
      const li = document.createElement("li");
      li.textContent = b;
      return li;
    }),
  );
  dom.applyGo.disabled = blocked;

  openModal("apply");
}

async function confirmApply() {
  if (state.plan?.blockers?.length) return;
  closeModal();

  const report = await invoke("apply");
  // The assignments were consumed by the run; the collections stay, so a
  // second pass can sort into the same folders.
  state.assigned = state.assigned.map(() => null);
  renderStatus();

  dom.reportTitle.textContent = report.error ? "Aplicação interrompida" : "Aplicado";
  dom.reportBody.replaceChildren();
  const row = (count, what) => {
    const dt = document.createElement("dt");
    dt.textContent = String(count);
    const dd = document.createElement("dd");
    dd.textContent = what;
    dom.reportBody.append(dt, dd);
  };

  row(report.sidecars, ".xmp gravados");
  if (report.moved) {
    row(report.moved, `fotos movidas (${report.filesMoved} arquivos)`);
  }
  row(report.rejected, "rejeitadas — permanecem onde estão");
  row(report.untouched, "sem marcação, ficam como estão");

  if (report.error) {
    const dd = document.createElement("dd");
    dd.className = "failure";
    dd.textContent = `parou em ${report.error} — o restante ficou intacto`;
    dom.reportBody.append(dd);
  }
  openModal("report");
}

// ---------------------------------------------------------------- modals

function openModal(which) {
  closeModal();
  state.modal = which;
  dom[which].hidden = false;
}

function closeModal() {
  if (state.modal) dom[state.modal].hidden = true;
  state.modal = null;
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
}

/// Keys a modal claims for itself. A mode with no entry here swallows
/// everything except Escape, which is what the new-collection field needs:
/// its keys have to reach the input, not the culling shortcuts.
const MODAL_KEYS = {
  move(e) {
    if (e.key >= "0" && e.key <= "9") {
      e.preventDefault();
      const digit = Number(e.key);
      if (digit === 0) return chooseCollection(null);
      if (digit <= state.collections.length) chooseCollection(digit - 1);
    }
  },
  apply(e) {
    if (e.key === "Enter") {
      e.preventDefault();
      confirmApply();
    }
  },
};

async function loadSettings() {
  const settings = await invoke("get_settings");
  for (const input of document.querySelectorAll('input[name="compat"]')) {
    input.checked = input.value === settings.xmpCompat;
    input.addEventListener("change", () => {
      if (input.checked) {
        invoke("set_settings", { settings: { xmpCompat: input.value } }).catch(() => {});
      }
    });
  }
}

// -------------------------------------------------------------- keyboard

document.addEventListener("keydown", (e) => {
  // `event.key`, never `event.code`: with Colemak-DH in the OS, the R key
  // reports `KeyS`, because that is its QWERTY position.
  const key = e.key;

  if (state.modal) {
    if (key === "Escape") {
      e.preventDefault();
      closeModal();
      return;
    }
    MODAL_KEYS[state.modal]?.(e);
    return;
  }

  if (e.ctrlKey || e.metaKey) {
    const lower = key.toLowerCase();
    if (lower === "z") {
      e.preventDefault();
      applyJump(invoke(e.shiftKey ? "redo" : "undo"));
    } else if (key === "Enter") {
      e.preventDefault();
      openApply();
    }
    return;
  }

  if (key === "o" || key === "O") {
    e.preventDefault();
    pickFolder();
    return;
  }
  if (key === "c" || key === "C") {
    e.preventDefault();
    openModal("settings");
    return;
  }
  if (key === "?") {
    e.preventDefault();
    openModal("help");
    return;
  }

  if (!state.photos.length) return;
  const index = state.index;

  switch (key) {
    case "h":
    case "H":
      e.preventDefault();
      goto(index + 1, performance.now());
      return;
    case "k":
    case "K":
      e.preventDefault();
      goto(index - 1, performance.now());
      return;
    case " ":
      e.preventDefault();
      applyChange(invoke("toggle_label", { index }), dom.label);
      return;
    case "Backspace":
      e.preventDefault();
      // Rejection is the only mark that advances, because it is terminal:
      // there is nothing else to decide about this frame.
      applyChange(invoke("toggle_reject", { index }), dom.reject);
      goto(index + 1);
      return;
    case "n":
    case "N":
      e.preventDefault();
      startNewCollection();
      return;
    case "m":
    case "M":
      e.preventDefault();
      startMove();
      return;
  }

  const stars = STARS[key.toLowerCase()];
  if (stars !== undefined) {
    e.preventDefault();
    // A rating never advances. The user rates, looks again, then moves on.
    applyChange(invoke("set_star", { index, stars }), dom.rating);
  }
});

// ----------------------------------------------------------------- setup

dom.stage.addEventListener("contextmenu", (e) => e.preventDefault());
el("open").addEventListener("click", pickFolder);
el("config").addEventListener("click", () => openModal("settings"));
dom.applyGo.addEventListener("click", confirmApply);

dom.collectionName.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    confirmNewCollection();
  }
});

new ResizeObserver(() => {
  for (const slot of state.slots) {
    if (slot.index !== null) fit(slot, state.photos[slot.index]);
  }
}).observe(dom.stage);

loadSettings();
