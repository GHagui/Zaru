"use strict";

// `t`, `tc` and `tm` are published by ui/i18n.js, which loads first.

// The navigation loop lives entirely on this side. A keypress must not wait on
// the command channel, on disk, or on a JPEG decoder: by the time the user
// presses H, the next photo is already decoded and sitting in the DOM, and the
// key does nothing but change which element is visible.

const { invoke, convertFileSrc } =
  window.__TAURI__?.core ?? window.__TAURI_INTERNALS__ ?? {};

/// Frames kept decoded ahead of and behind the current one. The Rust pool is
/// told which frames those are rather than inferring them, so this is the only
/// place the window is defined.
const AHEAD = 5;
const BEHIND = 3;
/// Two more elements than the window, so compare mode can hold a pinned frame
/// and a spare without evicting anything the pass is about to need.
const RING = AHEAD + BEHIND + 3;

/// Held down, an action applies to the whole burst instead of one frame.
///
/// Alt and not Shift, because Shift changes `event.key` itself: a collection
/// bound to `;` would arrive as `:` and match nothing. Alt leaves the key alone
/// whatever it is, so every binding works the same way.
const BURST_MODIFIER = "altKey";

/// `xmp:Label` stores the colour under its standard English name, which is
/// what every other program expects to read. The chip shows it in Portuguese.
const LABEL_NAMES = { Green: "verde" };

/// How often the marks are copied to the crash-recovery file. Not on every
/// keystroke: two thousand marks is a hundred kilobytes, and that write has no
/// business inside the culling loop.
const CHECKPOINT_MS = 3000;

const el = (id) => document.getElementById(id);
const dom = {
  folder: el("folder"),
  filterChip: el("filter-chip"),
  burst: el("burst"),
  counter: el("counter"),
  stage: el("stage"),
  ring: el("ring"),
  divider: el("divider"),
  pinTag: el("pin-tag"),
  empty: el("empty"),
  emptyMsg: el("empty-msg"),
  rating: el("rating"),
  label: el("label"),
  reject: el("reject"),
  collection: el("collection"),
  name: el("name"),
  zoom: el("zoom"),
  latency: el("latency"),
  recovery: el("recovery"),
  recoveryBody: el("recovery-body"),
  recoveryGo: el("recovery-go"),
  filter: el("filter"),
  filterList: el("filter-list"),
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
  keymapList: el("keymap"),
  keymapError: el("keymap-error"),
  keymapReset: el("keymap-reset"),
  help: el("help"),
  helpKeys: el("help-keys"),
};

/// What a filter lets through. Order is the order of the digits in the picker.
const FILTERS = [
  { name: "tudo", keep: () => true },
  { name: t("filter.unmarked"), keep: (m, c) => m.rating === 0 && !m.label && c === null },
  { name: t("filter.rated"), keep: (m) => m.rating > 0 },
  { name: t("filter.fiveStars"), keep: (m) => m.rating === 5 },
  { name: "verde", keep: (m) => !!m.label },
  { name: "rejeitadas", keep: (m) => m.rating === -1 },
  { name: t("filter.inCollection"), keep: (m, c) => c !== null },
  { name: t("filter.noCollection"), keep: (m, c) => c === null },
];

const state = {
  photos: [],
  marks: [],
  collections: [],
  assigned: [],
  /// Photo indices in pass order. A filter makes this a subset, and every
  /// movement is a step through it rather than through the file list.
  visible: [],
  at: 0,
  filter: 0,
  modal: null,
  plan: null,
  pinned: null,
  view: { scale: 1, x: 0, y: 0 },
  slots: [],
  /// Bindings, and the reverse lookup the key handler actually uses.
  keymap: null,
  actionLabels: new Map(),
  byKey: new Map(),
  /// The action waiting to be given a key in the settings screen.
  capturing: null,
  busy: false,
  pendingXmp: [],
  applyRequest: {},
  collectionCounts: [],
  returnFocus: null,
};

const current = () => state.visible[state.at];

// ---------------------------------------------------------------- ring

// Each frame gets a clipping box of its own. Without it a zoomed image spills
// far past its pane — at 100% it is four thousand pixels wide — and in compare
// mode the two frames simply paint over each other.
for (let i = 0; i < RING; i++) {
  const box = document.createElement("div");
  box.className = "frame";
  const img = document.createElement("img");
  img.draggable = false;
  box.appendChild(img);
  dom.ring.appendChild(box);
  state.slots.push({ el: img, box, index: null });
}

function mediaUrl(index) {
  return convertFileSrc(`media/${index}`, "zaru");
}

const isVideo = (index) => state.photos[index]?.kind === "video";

function frameUrl(index) {
  return convertFileSrc(String(index), "zaru");
}

/// Places `index` in the ring and starts decoding it. Returns its slot.
///
/// The victim is whatever the pass is furthest from, and never the pinned
/// frame, which has to survive however far the comparison wanders.
function assignSlot(index) {
  const held = state.slots.find((s) => s.index === index);
  if (held) return held;

  let victim = state.slots.find((s) => s.index === null);
  if (!victim) {
    let worst = -1;
    for (const slot of state.slots) {
      if (slot.index === state.pinned) continue;
      const distance = Math.abs(state.visible.indexOf(slot.index) - state.at);
      if (distance > worst) {
        worst = distance;
        victim = slot;
      }
    }
  }

  victim.index = index;
  victim.el.alt = state.photos[index]?.name ?? "";
  victim.el.src = frameUrl(index);
  place(victim);
  // Decoding ahead of time is the whole point: without it the keypress pays
  // for a full-resolution JPEG decode and the loop stutters.
  victim.el.decode().catch(() => {});
  return victim;
}

// Reserve one existing ring element for the reference, including when the current photo is the reference.
const reference = state.slots.pop();

// ------------------------------------------------------------ geometry

/// The box a frame is drawn into. One pane normally; two side by side while a
/// frame is pinned for comparison.
function pane(role) {
  const w = dom.stage.clientWidth;
  const h = dom.stage.clientHeight;
  if (state.pinned === null) return { w, h, cx: w / 2 };
  const half = (w - 3) / 2;
  return { w: half, h, cx: role === "pinned" ? half / 2 : half + 3 + half / 2 };
}

/// Screen size of the photo itself inside its pane at scale 1, and the scale
/// that would show it at one image pixel per screen pixel.
function geometry(photo, box) {
  const turned = photo.rotation === 90 || photo.rotation === 270;
  const boxW = turned ? box.h : box.w;
  const boxH = turned ? box.w : box.h;
  const unit = Math.min(boxW / photo.width, boxH / photo.height);
  const w = photo.width * unit;
  const h = photo.height * unit;
  return {
    w: turned ? h : w,
    h: turned ? w : h,
    native: unit > 0 ? 1 / unit : 1,
  };
}

/// How far the frame may be dragged before its edge would come inside the pane.
function panLimit(photo, box, scale) {
  const size = geometry(photo, box);
  return {
    x: Math.max(0, (size.w * scale - box.w) / 2),
    y: Math.max(0, (size.h * scale - box.h) / 2),
  };
}

function clampPan() {
  const photo = state.photos[current()];
  if (!photo) return;
  const limit = panLimit(photo, pane("current"), state.view.scale);
  state.view.x = Math.max(-limit.x, Math.min(limit.x, state.view.x));
  state.view.y = Math.max(-limit.y, Math.min(limit.y, state.view.y));
}

/// Sizes and positions one slot's element.
///
/// The stage never changes size, and neither does the pane: portrait and
/// landscape land in the same box, so the user keeps their spatial reference
/// between frames. Zoom scales the content inside that box, never the box.
function place(slot, role) {
  const photo = state.photos[slot.index];
  if (!photo) return;
  role = role ?? (slot.index === state.pinned ? "pinned" : "current");
  const box = pane(role);
  const turned = photo.rotation === 90 || photo.rotation === 270;

  slot.box.style.left = `${box.cx - box.w / 2}px`;
  slot.box.style.width = `${box.w}px`;
  slot.el.style.width = `${turned ? box.h : box.w}px`;
  slot.el.style.height = `${turned ? box.w : box.h}px`;

  const { scale, x, y } = state.view;
  slot.el.style.transform =
    `translate(-50%, -50%) translate(${x}px, ${y}px) scale(${scale}) ` +
    `rotate(${photo.rotation}deg)` +
    (photo.mirrored ? " scaleX(-1)" : "");
}

function placeShown() {
  for (const slot of [...state.slots, reference]) {
    if (slot.box.classList.contains("shown")) place(slot);
  }
}

// ------------------------------------------------------------ rendering

function show() {
  if (window.gridUI?.active) { window.gridUI.render(); renderStatus(); return; }
  if (!state.visible.length) {
    for (const slot of [...state.slots, reference]) slot.box.classList.remove("shown");
    renderStatus();
    return;
  }
  // A clip has no still to hand over, so the ring stands down and the player
  // takes the pane.
  const video = isVideo(current());
  const playerBox = el("player-box");
  const player = el("player");
  playerBox.hidden = !video;
  if (video) {
    const wanted = mediaUrl(current());
    if (player.dataset.src !== wanted) {
      player.dataset.src = wanted;
      player.src = wanted;
    }
    const box = pane("current");
    playerBox.style.left = `${box.cx - box.w / 2}px`;
    playerBox.style.width = `${box.w}px`;
    for (const s of [...state.slots, reference]) s.box.classList.remove("shown");
    renderStatus();
    return;
  }
  if (player.dataset.src) {
    // Leaving a clip stops it; a video still playing behind a photo would be
    // sound with no picture.
    player.pause();
    player.removeAttribute("src");
    delete player.dataset.src;
    player.load();
  }

  const slot = assignSlot(current());
  const pinnedSlot = state.pinned === null ? null : reference;
  if (pinnedSlot && pinnedSlot.index !== state.pinned) {
    pinnedSlot.index = state.pinned;
    pinnedSlot.el.src = frameUrl(state.pinned);
    pinnedSlot.el.alt = state.photos[state.pinned].name;
    pinnedSlot.el.decode().catch(() => {});
  }

  for (const s of [...state.slots, reference]) {
    const role = s === pinnedSlot ? "pinned" : s === slot ? "current" : null;
    s.box.classList.toggle("shown", role !== null);
    if (role) place(s, role);
  }
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
  const index = current();
  const photo = state.photos[index];
  const mark = state.marks[index];
  const hasPhoto = !!photo;
  document.body.classList.toggle("no-results", !!state.photos.length && !hasPhoto);
  el("no-results").hidden = !state.photos.length || hasPhoto;
  dom.counter.textContent = `${hasPhoto ? state.at + 1 : 0} / ${state.visible.length}`;
  dom.name.textContent = photo?.name ?? (state.photos.length ? t("apply.scopeNone") : t("footer.idle"));
  dom.name.title = photo?.name ?? "";
  dom.burst.textContent = photo?.burstSize > 1
    ? t("burst.position", { index: photo.burstIndex + 1, size: photo.burstSize })
    : "";
  dom.filterChip.hidden = state.filter === 0;
  dom.filterChip.textContent = state.filter ? `${FILTERS[state.filter].name} · ${state.visible.length} de ${state.photos.length}` : "";
  const rejected = mark?.rating === -1;
  const stars = Math.max(0, mark?.rating ?? 0);
  dom.rating.querySelectorAll("button").forEach((cell, i) => {
    cell.classList.toggle("on", i < stars);
    cell.setAttribute("aria-pressed", String(stars === i + 1));
  });
  dom.reject.setAttribute("aria-pressed", String(rejected));
  dom.reject.textContent = rejected ? t("mark.rejected") : t("mark.reject");
  dom.label.setAttribute("aria-pressed", String(!!mark?.label));
  const collection = state.assigned[index];
  dom.collection.hidden = collection === null || collection === undefined;
  if (!dom.collection.hidden) {
    const name = state.collections[collection];
    dom.collection.textContent = name;
    dom.collection.title = name;
    dom.collection.style.setProperty("--chip", chipColour(name));
  }
  dom.zoom.textContent = hasPhoto && state.view.scale !== 1
    ? t("action.zoomAt", { percent: Math.round(state.view.scale / geometry(photo, pane("current")).native * 100) })
    : t("action.zoom");
  const comparing = state.pinned !== null && hasPhoto;
  dom.divider.hidden = !comparing;
  dom.pinTag.hidden = !comparing;
  el("current-tag").hidden = !comparing;
  dom.pinTag.textContent = comparing ? t("compare.reference", { name: state.photos[state.pinned].name }) : "";
  el("current-tag").textContent = comparing ? t("compare.current", { name: photo.name }) : "";
  document.querySelectorAll('[data-command="compare"]').forEach(b => b.setAttribute("aria-pressed", String(comparing)));
  document.querySelectorAll("[data-photo]").forEach(b => b.disabled = !hasPhoto || state.busy);
  // Zoom and side-by-side belong to stills. The guard already refuses the
  // action; the button has to say so too, or it looks clickable and does
  // nothing, which is worse than being plainly off.
  const still = hasPhoto && !isVideo(index);
  document.querySelectorAll('[data-command="zoom"], [data-command="compare"]')
    .forEach(b => b.disabled = !still || state.busy);

  document.querySelectorAll("[data-session]").forEach(b => b.disabled = !state.photos.length || state.busy);
  document.querySelectorAll('[data-command="prev"]').forEach(b => b.disabled = !hasPhoto || state.at === 0 || state.busy);
  document.querySelectorAll('[data-command="next"]').forEach(b => b.disabled = !hasPhoto || state.at === state.visible.length - 1 || state.busy);
  document.querySelectorAll("#sidebar-list [data-collection]").forEach(b => {
    b.setAttribute("aria-pressed", String(Number(b.dataset.collection) === collection));
    b.disabled = !hasPhoto || state.busy;
  });
  window.gridUI?.renderStatus();
  renderExif();
}

/// Exif records no time zone, and `captured` was built as if the wall clock the
/// camera showed were UTC. Formatting in the machine's zone would slide every
/// timestamp by the local offset — three hours in Brazil — and the caption
/// would then lie about when the shutter fired.
const UTC = { timeZone: "UTC" };

function gridTimestamp(ms) {
  if (ms == null) return "";
  const d = new Date(ms);
  const two = (n) => String(n).padStart(2, "0");
  return `${two(d.getUTCDate())}/${two(d.getUTCMonth() + 1)} ${two(d.getUTCHours())}:${two(d.getUTCMinutes())}:${two(d.getUTCSeconds())}`;
}

function fullTimestamp(ms) {
  const d = new Date(ms);
  return d.toLocaleString("pt-BR", { ...UTC, dateStyle: "short", timeStyle: "medium" });
}

/// A clip's length, the way a player shows it.
function duration(ms) {
  if (ms == null) return "";
  const total = Math.round(ms / 1000);
  const [h, m, sec] = [Math.floor(total / 3600), Math.floor((total % 3600) / 60), total % 60];
  const pad = (n) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}

window.zaruTime = { gridTimestamp, fullTimestamp, duration };

function goto(position, pressedAt) {
  if (!state.visible.length || state.busy) return;
  const next = Math.max(0, Math.min(state.visible.length - 1, position));
  // At either end the key does nothing, and timing a frame that never changed
  // would quietly flatter the measurement.
  if (next === state.at) return;
  if (pressedAt !== undefined) measureFrom(pressedAt);
  state.at = next;
  clampPan();
  show();
  fillRing();
}

const burstAt = (position) => state.photos[state.visible[position]].burst;

/// Walks back to the first visible frame of whichever burst `position` is in.
function burstStart(position) {
  while (position > 0 && burstAt(position - 1) === burstAt(position)) position--;
  return position;
}

/// Jumps to the first frame of the next or previous burst.
///
/// In a motorsport pass the unit of decision is the burst, not the frame: you
/// want one photo of that car in that corner, not a verdict on all twelve.
/// Going back lands on the start of the current burst first, the way a track
/// skip does, and only then on the one before it.
function gotoBurst(direction) {
  if (!state.visible.length) return;
  const here = burstAt(state.at);

  if (direction > 0) {
    let position = state.at;
    while (position + 1 < state.visible.length) {
      position++;
      if (burstAt(position) !== here) return goto(position, performance.now());
    }
    return;
  }

  const start = burstStart(state.at);
  if (start < state.at) return goto(start, performance.now());
  if (start === 0) return;
  goto(burstStart(start - 1), performance.now());
}

function windowOrder() {
  const out = [];
  for (let d = 0; d <= AHEAD; d++) {
    if (state.at + d < state.visible.length) out.push(state.visible[state.at + d]);
  }
  for (let d = 1; d <= BEHIND; d++) {
    if (state.at - d >= 0) out.push(state.visible[state.at - d]);
  }
  if (state.visible.length && state.pinned !== null && !out.includes(state.pinned)) out.unshift(state.pinned);
  return out;
}

let focusPending = null;
function fillRing() {
  if (window.gridUI?.active) return;
  const frames = windowOrder();
  for (const i of frames) assignSlot(i);
  // Tell the Rust pool which frames to keep, at most once per painted frame.
  if (focusPending === null) {
    focusPending = requestAnimationFrame(() => {
      focusPending = null;
      invoke("focus", { frames: windowOrder() }).catch(() => {});
    });
  }
}

// ---------------------------------------------------------------- zoom

/// Zoom is kept across frames on purpose. Zooming to where the autofocus point
/// was and then walking a burst at 100% is the whole reason it exists — losing
/// it on every keypress would make it useless for exactly that.
/// Zoom and side-by-side belong to stills.
///
/// A clip has no fixed frame to magnify and no still to compare against, and a
/// control that pretends otherwise is worse than one that says no.
const stillOnly = () => state.photos.length && !isVideo(current());

function setScale(next, originX, originY) {
  if (!stillOnly()) return;
  const photo = state.photos[current()];
  if (!photo) return;
  const native = geometry(photo, pane("current")).native;
  const clamped = Math.max(1, Math.min(native, next));
  if (clamped === state.view.scale) return;

  // Keep whatever is under the cursor under the cursor.
  const ratio = clamped / state.view.scale;
  state.view.x = originX - (originX - state.view.x) * ratio;
  state.view.y = originY - (originY - state.view.y) * ratio;
  state.view.scale = clamped;

  clampPan();
  placeShown();
  renderStatus();
}

function resetView() {
  if (state.view.scale === 1 && state.view.x === 0 && state.view.y === 0) return;
  state.view = { scale: 1, x: 0, y: 0 };
  placeShown();
  renderStatus();
}

function toggleNative() {
  if (!stillOnly()) return;
  const photo = state.photos[current()];
  if (!photo) return;
  const native = geometry(photo, pane("current")).native;
  if (state.view.scale > 1) return resetView();
  setScale(native, 0, 0);
}

// ------------------------------------------------------------- compare

/// Pins the current frame beside the pass. Both halves share one zoom and one
/// pan, which is what makes the comparison mean anything: the same corner of
/// two frames, at the same magnification.
function togglePin() {
  if (!stillOnly()) return;
  state.pinned = state.pinned === null ? current() : null;
  document.body.classList.toggle("compare", state.pinned !== null);
  dom.divider.hidden = state.pinned === null;
  dom.pinTag.hidden = state.pinned === null;
  clampPan();
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
  state.pendingXmp[change.index] = change.pendingXmp ?? true;
  state.assigned[change.index] = change.collection;
  updateCollectionCounts();
  renderStatus();
  if (change.index !== current()) return;
  flash(confirmOn);
}

/// A short static outline confirms a change without animating the photograph.
function flash(node) {
  if (!node || node.hidden) return;
  node.classList.add("flash");
  setTimeout(() => node.classList.remove("flash"), 120);
}

/// Undo and redo have to bring the photo they repaired back into view, or the
/// user cannot see what changed. A filter may be hiding it, in which case the
/// filter has to give way — the repair matters more than the view.
async function applyJump(promise) {
  const changes = await promise;
  if (!changes?.length) return;
  for (const c of changes) {
    state.marks[c.index] = c.mark;
    state.pendingXmp[c.index] = c.pendingXmp ?? true;
    state.assigned[c.index] = c.collection;
  }
  updateCollectionCounts();
  // Land on the first photo the step touched.
  if (window.gridUI?.active) { window.gridUI.render(); renderStatus(); return; }
  const change = changes[0];

  let position = state.visible.indexOf(change.index);
  if (position < 0) {
    setFilter(0);
    position = state.visible.indexOf(change.index);
  }
  if (position < 0 || position === state.at) {
    renderStatus();
    return;
  }
  state.at = position;
  clampPan();
  show();
  fillRing();
}

function adopt(session) {
  state.photos = session.photos;
  state.marks = session.marks;
  state.collections = session.collections;
  state.assigned = session.assigned;
  state.pendingXmp = session.pendingXmp ?? session.marks.map(() => false);
  updateCollectionCounts();
}

async function openFolder(path) {
  const session = await invoke("open_folder", { path });
  adopt(session);
  window.gridUI?.reset();
  state.filter = 0;
  state.pinned = null;
  state.at = 0;
  state.view = { scale: 1, x: 0, y: 0 };
  document.body.classList.remove("compare");
  dom.divider.hidden = true;
  dom.pinTag.hidden = true;
  rebuildVisible(0);

  for (const slot of [...state.slots, reference]) {
    slot.index = null;
    slot.el.removeAttribute("src");
    slot.box.classList.remove("shown");
  }
  dom.folder.textContent = session.folder.split(/[\\\\/]/).filter(Boolean).pop();
  dom.folder.title = session.folder;
  dom.empty.hidden = true;
  document.body.classList.remove("idle");
  samples.length = 0;
  show();
  fillRing();
  dom.stage.focus();
  await offerRecovery();
}

async function pickFolder() {
  if (state.busy) return;
  setBusy(true, t("status.opening"));
  try {
    const path = await invoke("pick_folder");
    if (!path) return;
    await openFolder(path);
  } catch (e) {
    notifyUser(tm(e), true);
  } finally {
    setBusy(false);
  }
}

// --------------------------------------------------------------- filter

/// Recomputes the pass, keeping the photo the user is looking at if the new
/// filter still lets it through, and landing on the nearest one if not.
function rebuildVisible(keepIndex) {
  const test = FILTERS[state.filter].keep;
  state.visible = [];
  for (let i = 0; i < state.photos.length; i++) {
    if (test(state.marks[i], state.assigned[i] ?? null)) state.visible.push(i);
  }
  const exact = state.visible.indexOf(keepIndex);
  state.at =
    exact >= 0
      ? exact
      : Math.max(0, state.visible.findIndex((i) => i >= keepIndex));
}

function setFilter(which) {
  state.filter = which;
  rebuildVisible(current());
  window.gridUI?.filterChanged();
  clampPan();
  show();
  fillRing();
}

function startFilter() {
  dom.filterList.replaceChildren();
  FILTERS.forEach((filter, i) => {
    const count = state.photos.filter((_, p) =>
      filter.keep(state.marks[p], state.assigned[p] ?? null),
    ).length;

    const li = document.createElement("li");
    const key = document.createElement("kbd");
    key.textContent = showKey(pickerKey(i));
    const label = document.createElement("span");
    label.textContent = filter.name;
    if (i === state.filter) label.className = "chosen";
    const tally = document.createElement("b");
    tally.textContent = tc("count.photos", count);
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("aria-pressed", String(i === state.filter));
    button.append(key, label, tally);
    button.addEventListener("click", () => { closeModal(); setFilter(i); });
    li.append(button);
    dom.filterList.append(li);
  });
  openModal("filter");
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
  if (state.busy) return;
  setBusy(true);
  try {
    state.collections = await invoke("new_collection", {
      name: dom.collectionName.value,
    });
    updateCollectionCounts();
    closeModal();
    // Straight into the picker: creating a collection is nearly always the
    // first half of putting this photo in it.
    startMove();
  } catch (e) {
    dom.collectionError.textContent = String(e);
    dom.collectionError.hidden = false;
    dom.collectionName.focus();
  } finally {
    setBusy(false);
  }
}

/// `M` is a two-keystroke mode: the list opens, a digit picks, and it closes.
function startMove() {
  dom.moveList.replaceChildren();

  if (!state.collections.length) {
    const li = document.createElement("li");
    const p = document.createElement("span");
    p.className = "none";
    p.textContent = t("move.none");
    li.append(p);
    dom.moveList.append(li);
  } else {
    const counts = state.collections.map(
      (_, c) => state.assigned.filter((a) => a === c).length,
    );
    state.collections.forEach((name, c) => {
      const li = document.createElement("li");

      const key = document.createElement("kbd");
      key.textContent = showKey(pickerKey(c));

      const label = document.createElement("span");
      const chip = document.createElement("span");
      chip.className = "chip chip-collection";
      chip.textContent = name;
      chip.style.setProperty("--chip", chipColour(name));
      label.append(chip);

      const count = document.createElement("b");
      count.textContent = tc("count.photos", counts[c]);
      if (!counts[c]) count.textContent = "";

      const button = document.createElement("button");
      button.append(key, label, count);
      button.setAttribute("aria-pressed", String(state.assigned[current()] === c));
      button.addEventListener("click", () => chooseCollection(c).catch(reportError));
      li.append(button);
      dom.moveList.append(li);
    });
  }
  openModal("move");
}

function chooseCollection(collection) {
  if (window.gridUI?.active) {
    closeModal();
    return window.gridUI.edit({ kind: "collection", value: collection });
  }
  if (!state.visible.length || state.busy) return Promise.resolve();
  closeModal();
  return applyChange(
    invoke("assign", { index: current(), collection }),
    dom.collection,
  );
}

/// The key that picks the nth item in any list. The collections row doubles as
/// a generic picker row, because a 42-key split has no number row and every
/// list in this app is short.
function pickerKey(i) {
  return state.keymap?.collections?.[i] ?? String(i + 1);
}

/// Puts the current photo — or its whole burst — in a collection.
///
/// This is the hot path of a sorting pass, so it goes straight from the key to
/// the command: no picker, nothing drawn over the photo being judged. Pressing
/// the key of the collection a photo is already in takes it out again, the same
/// way pressing a rating it already has clears it.
function assignTo(collection, wholeBurst) {
  const index = current();
  const target = state.assigned[index] === collection ? null : collection;
  if (!wholeBurst) {
    return applyChange(invoke("assign", { index, collection: target }), dom.collection);
  }
  return applyMany(invoke("assign_burst", { index, collection: target }));
}

/// Applies a change that touched several photos at once, without moving.
async function applyMany(promise) {
  const changes = await promise;
  if (!changes?.length) return;
  for (const change of changes) {
    state.marks[change.index] = change.mark;
    state.pendingXmp[change.index] = change.pendingXmp ?? true;
    state.assigned[change.index] = change.collection;
  }
  updateCollectionCounts();
  renderStatus();
  flash(dom.collection);
}

/// A collection key pressed before that collection exists.
function flashCollectionHint(which) {
  notifyUser(t("collections.missing", { number: which + 1 }));
}

// ---------------------------------------------------------------- apply

function tallyRow(into, count, what, note) {
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
  into.append(dt, dd);
}

/// Shows what Apply would do before it does any of it. Moving files is the one
/// irreversible thing in the app, and it should never be a surprise.
async function openApply(operation = "both", indices = null) {
  if (state.busy || !state.photos.length) return;
  setBusy(true, t("status.preparing"));
  let plan;
  state.applyRequest = { operation, indices: indices === null ? null : [...indices] };
  try { plan = await invoke("plan", state.applyRequest); } finally { setBusy(false); }
  state.plan = plan;
  const scope = indices === null ? t("apply.scopeAll") : tc("apply.scopeSelected", indices.length);
  el("apply-scope").textContent = `${scope} · ${operation === "xmp" ? t("apply.scopeXmp") : operation === "organization" ? t("apply.scopeOrganisation") : t("apply.scopeBoth")}`;

  dom.applyBody.replaceChildren();
  tallyRow(dom.applyBody, plan.evaluated, t("tally.evaluated"));
  tallyRow(dom.applyBody, plan.sidecars, t("tally.sidecars"));
  for (const move of plan.moves) {
    // The sidecar and the RAW+JPEG twin travel with the photo, so the file
    // count is usually higher than the photo count.
    const note = move.files !== move.photos ? tc("count.files", move.files) : "";
    tallyRow(dom.applyBody, move.photos, t("tally.movingTo", { collection: move.collection }), note);
  }
  // Rejected is a note in the metadata and nothing more. Zaru never deletes.
  tallyRow(dom.applyBody, plan.rejected, t("tally.rejected"));
  tallyRow(dom.applyBody, plan.untouched, t("tally.untouched"));

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
  if (state.busy || !state.plan || state.plan.blockers.length) return;
  setBusy(true, t("status.applying"));
  let report;
  try {
    report = await invoke("apply", state.applyRequest);
  } catch (e) {
    reportError(e);
    return;
  } finally {
    setBusy(false);
  }
  closeModal();
  // The assignments were consumed by the run; the collections stay, so a
  // second pass can sort into the same folders.
  if (report.session) adopt(report.session);
  else if (!report.error && state.applyRequest.operation !== "xmp") {
    const done = new Set(state.applyRequest.indices ?? state.photos.map((_, i) => i));
    state.assigned = state.assigned.map((value, i) => done.has(i) ? null : value);
  }
  window.gridUI?.invalidate();
  updateCollectionCounts();
  rebuildVisible(current());
  window.gridUI?.filterChanged();
  show();

  dom.reportTitle.textContent = report.error ? t("report.stopped") : t("report.done");
  dom.reportBody.replaceChildren();
  tallyRow(dom.reportBody, report.sidecars, ".xmp gravados");
  if (report.moved) {
    tallyRow(dom.reportBody, report.moved, t("tally.moved", { files: tc("count.files", report.filesMoved) }));
  }
  tallyRow(dom.reportBody, report.rejected, t("tally.rejected"));
  tallyRow(dom.reportBody, report.untouched, t("tally.untouched"));

  if (report.error) {
    const dd = document.createElement("dd");
    dd.className = "failure";
    dd.textContent = `parou em ${report.error} — o restante ficou intacto`;
    dom.reportBody.append(dd);
  }
  openModal("report");
}

// -------------------------------------------------------------- recovery

async function offerRecovery() {
  const offer = await invoke("recovery_offer");
  if (!offer) return;
  dom.recoveryBody.replaceChildren();
  tallyRow(dom.recoveryBody, offer.marked, t("tally.marked"));
  if (offer.assigned) tallyRow(dom.recoveryBody, offer.assigned, t("tally.assigned"));
  if (offer.collections) tallyRow(dom.recoveryBody, offer.collections, t("tally.collections"));
  openModal("recovery");
}

async function restoreSession() {
  if (state.busy) return;
  setBusy(true, t("status.restoring"));
  try {
    const session = await invoke("restore_session");
    if (!session) throw new Error(t("recovery.gone"));
    adopt(session);
    rebuildVisible(current());
    closeModal();
    show();
    fillRing();
  } finally { setBusy(false); }
}

async function discardRecovery() {
  if (state.busy) return;
  setBusy(true);
  try { await invoke("discard_recovery"); closeModal(); }
  finally { setBusy(false); }
}

// ---------------------------------------------------------------- modals

function openModal(which) {
  const trigger = state.returnFocus ?? document.activeElement;
  closeModal();
  state.returnFocus = trigger;
  state.modal = which;
  dom[which].hidden = false;
  el("backdrop").hidden = false;
  document.querySelectorAll("body > header, body > main, body > footer, body > aside").forEach(n => n.inert = true);
  const initial = which === "newCollection" ? dom.collectionName
    : which === "apply" ? (dom.applyGo.disabled ? dom[which].querySelector("[data-close]") : dom.applyGo)
    : dom[which].querySelector("button:not([disabled]), input, [tabindex]");
  (initial ?? dom[which]).focus();
}

function closeModal() {
  if (state.capturing) stopCapture();
  if (state.modal) dom[state.modal].hidden = true;
  state.modal = null;
  el("backdrop").hidden = true;
  document.querySelectorAll("body > header, body > main, body > footer, body > aside").forEach(n => n.inert = false);
  const trigger = state.returnFocus;
  state.returnFocus = null;
  if (trigger instanceof HTMLElement && trigger.isConnected && !trigger.closest("[hidden]") && !trigger.disabled) trigger.focus();
}

/// Keys a modal claims for itself. A mode with no entry here swallows
/// everything except Escape, which is what the new-collection field needs:
/// its keys have to reach the input, not the culling shortcuts.
/// Which row of a picker a key selects, or -1.
function pickedRow(e, rows) {
  const key = normalise(e.key);
  for (let i = 0; i < rows; i++) {
    if (normalise(pickerKey(i)) === key) return i;
  }
  return -1;
}

const MODAL_KEYS = {
  move(e) {
    if (normalise(e.key) === normalise(state.keymap.newCollection)) {
      e.preventDefault();
      return startNewCollection();
    }
    const row = pickedRow(e, state.collections.length);
    if (row >= 0) {
      e.preventDefault();
      chooseCollection(row).catch(reportError);
      return;
    }
    // The same key that opens the list clears the collection, so taking a photo
    // out never needs a key of its own.
    if (normalise(e.key) === normalise(state.keymap?.moveTo ?? "m")) {
      e.preventDefault();
      executeAction("unassign").catch(reportError);
    }
  },
  filter(e) {
    const row = pickedRow(e, FILTERS.length);
    if (row >= 0) {
      e.preventDefault();
      closeModal();
      setFilter(row);
    }
  },
  apply(e) {
    if (e.key === "Enter") {
      e.preventDefault();
      confirmApply().catch(reportError);
    }
  },
  recovery(e) {
    if (e.key === "Enter") {
      e.preventDefault();
      restoreSession().catch(reportError);
    }
  },
};

/// A binding is a `KeyboardEvent.key`, and a letter arrives uppercased when a
/// modifier is down. Comparing in lower case makes `Alt+Q` and `q` one key.
function normalise(key) {
  return key.length === 1 ? key.toLowerCase() : key;
}

function adoptKeymap(keymap) {
  state.keymap = keymap;
  state.byKey = new Map();
  const bind = (key, action) => { if (key) state.byKey.set(normalise(key), action); };

  for (const [action, key] of Object.entries(keymap)) {
    if (action === "collections") continue;
    bind(key, action);
  }
  keymap.collections.forEach((key, i) => bind(key, `collection${i + 1}`));

  renderKeymapEditor();
  renderHelp();
  renderShortcutHints();
  renderCollections();
}

/// The label for an action, including the collections, which are numbered
/// rather than named because a collection is created after the key exists.
function actionLabel(action) {
  const collection = action.match(/^collection(\d+)$/);
  if (collection) return t("settings.collectionKey", { number: collection[1] });
  return t(`keymapAction.${action}`);
}

function showKey(key) {
  if (key === " ") return t("key.space");
  return key;
}

function renderKeymapEditor() {
  if (!state.keymap) return;
  dom.keymapList.replaceChildren();

  const rows = [
    ...[...state.actionLabels.keys()].map((id) => [id, state.keymap[id]]),
    ...state.keymap.collections.map((key, i) => [`collection${i + 1}`, key]),
  ];

  for (const [action, key] of rows) {
    const row = document.createElement("button");
    row.className = "keyrow";
    row.dataset.action = action;

    const label = document.createElement("span");
    label.textContent = actionLabel(action);
    const cap = document.createElement("kbd");
    cap.textContent = showKey(key ?? "");

    row.append(label, cap);
    row.addEventListener("click", () => startCapture(action));
    dom.keymapList.append(row);
  }
}

function startCapture(action) {
  stopCapture();
  state.capturing = action;
  dom.keymapError.hidden = true;
  for (const row of dom.keymapList.children) {
    if (row.dataset.action !== action) continue;
    row.classList.add("capturing");
    row.querySelector("kbd").textContent = t("settings.pressKey");
  }
}

function stopCapture() {
  state.capturing = null;
  for (const row of dom.keymapList.children) row.classList.remove("capturing");
  renderKeymapEditor();
}

async function captureKey(key) {
  const action = state.capturing;
  stopCapture();
  try {
    adoptKeymap(await invoke("bind_key", { action, key }));
  } catch (e) {
    dom.keymapError.textContent = String(e);
    dom.keymapError.hidden = false;
  }
}

/// Fills the language menu and applies whichever language wins.
///
/// The system's choice is the default because nobody should have to configure
/// their own language; the menu exists for the case the system is wrong, which
/// is common for anyone whose Windows is in a language they do not prefer.
async function loadLanguages() {
  const list = await invoke("languages").catch(() => null);
  if (!list) return;
  state.extraLocales = list.extra ?? {};

  const available = [
    ...list.bundled.map((l) => l.tag),
    ...Object.keys(state.extraLocales),
  ];
  const wanted = list.chosen ?? matchLanguage(list.system, available) ?? window.zaruI18n.FALLBACK;
  await window.zaruI18n.useLanguage(wanted, state.extraLocales);

  const menu = el("language");
  menu.replaceChildren();
  const follow = new Option(t("settings.languageSystem"), "");
  follow.selected = !list.chosen;
  menu.append(follow);
  for (const tag of available) {
    const named = list.bundled.find((l) => l.tag === tag);
    const option = new Option(named ? named.name : tag, tag);
    option.selected = list.chosen === tag;
    menu.append(option);
  }
  el("language-folder").textContent = list.folder;

  menu.onchange = async () => {
    const chosen = menu.value || null;
    await invoke("set_language", { language: chosen }).catch(reportError);
    await window.zaruI18n.useLanguage(
      chosen ?? matchLanguage(list.system, available) ?? window.zaruI18n.FALLBACK,
      state.extraLocales,
    );
    retranslate();
  };
}

/// The closest available language to what the system asked for.
///
/// Region is dropped so `pt-PT` finds `pt-BR`, but script is kept: `zh-Hant`
/// must not quietly borrow `zh-Hans`, because a reader of one cannot read the
/// other.
function matchLanguage(requested, available) {
  if (!requested) return null;
  const parts = (tag) => {
    const bits = tag.toLowerCase().replace(/_/g, "-").split("-");
    return [bits[0], bits.find((b) => b.length === 4 && /^[a-z]+$/.test(b))];
  };
  const exact = available.find((a) => a.toLowerCase() === requested.toLowerCase());
  if (exact) return exact;
  const [language, script] = parts(requested);
  return available.find((a) => {
    const [theirs, theirScript] = parts(a);
    return theirs === language && (!script || !theirScript || script === theirScript);
  }) ?? null;
}

/// Redraws everything that was written in words rather than marked up.
function retranslate() {
  window.zaruI18n.translateDocument();
  renderStatus();
  renderKeymapEditor();
  renderHelp();
  window.gridUI?.render();
}

async function loadSettings() {
  await loadLanguages();
  const [settings, actions] = await Promise.all([
    invoke("get_settings"),
    invoke("key_actions"),
  ]);
  state.actionLabels = new Map(actions.map((id) => [id, t(`keymapAction.${id}`)]));
  adoptKeymap(settings.keymap);

  for (const input of document.querySelectorAll('input[name="compat"]')) {
    input.checked = input.value === settings.xmpCompat;
    input.addEventListener("change", () => {
      if (input.checked) {
        invoke("set_settings", {
          settings: { xmpCompat: input.value, keymap: state.keymap },
        }).catch(reportError);
      }
    });
  }
}

/// The help screen is generated, so it can never drift from what the keys
/// actually do — which is the whole risk once the map is editable.
function renderHelp() {
  if (!state.keymap) return;
  dom.helpKeys.replaceChildren();
  const row = (key, what) => {
    const dt = document.createElement("dt");
    dt.textContent = key;
    const dd = document.createElement("dd");
    dd.textContent = what;
    dom.helpKeys.append(dt, dd);
  };

  const k = state.keymap;
  row(showKey(k.grid || t("settings.noKey")), t("help.gridToggle"));
  row(t("help.gridKeys"), t("help.gridSelection"));
  row(showKey(k.prev), t("help.prev"));
  row(showKey(k.next), t("help.next"));
  row(`Alt+${showKey(k.prev)} / Alt+${showKey(k.next)}`, t("help.burst"));
  row(
    [k.star1, k.star2, k.star3, k.star4, k.star5].map(showKey).join(" "),
    t("help.stars"),
  );
  row(showKey(k.label), t("help.green"));
  row(showKey(k.reject), t("help.reject"));
  row(k.collections.map(showKey).join(" "), t("help.collections"));
  row(`Alt+${showKey(k.collections[0] ?? "")}`, t("help.burstAssign"));
  row(showKey(k.newCollection), t("help.newCollection"));
  row(showKey(k.moveTo), t("help.moveTo"));
  row(showKey(k.zoom), t("help.zoom"));
  row(showKey(k.compare), t("help.compare"));
  row(showKey(k.filter), t("help.filter"));
  row("Ctrl+Z / Ctrl+Shift+Z", t("help.undo"));
  row("Ctrl+Enter", t("help.apply"));
  row("Esc", t("help.escape"));
  row(showKey(k.open), t("help.open"));
  row(showKey(k.settings), t("help.settings"));
  row(showKey(k.help), t("help.help"));
}

// -------------------------------------------------------------- keyboard

async function executeAction(action, whole = false) {
  if (state.busy) return;
  switch (action) {
    case "grid": return window.gridUI?.toggle();
    case "photo": return window.gridUI?.toggle(false);
    case "open": return pickFolder();
    case "settings": return openModal("settings");
    case "help": return openModal("help");
    case "clearFilter": return setFilter(0);
    case "collections": return toggleCollections();
    case "exif": return toggleExif();
  }
  if (!state.photos.length) return;
  if (window.gridUI?.active && window.gridUI.actions.has(action)) return window.gridUI.action(action);
  switch (action) {
    case "apply": return openApply();
    case "filter": return startFilter();
    case "newCollection": return startNewCollection();
    case "moveTo": return startMove();
    case "undo": return applyJump(invoke("undo"));
    case "redo": return applyJump(invoke("redo"));
  }
  if (!state.visible.length) return;
  const index = current();
  const collection = action?.match(/^collection(\d+)$/);
  if (collection) {
    const which = Number(collection[1]) - 1;
    if (which >= state.collections.length) return flashCollectionHint(which);
    return assignTo(which, whole);
  }
  switch (action) {
    case "next": return whole ? gotoBurst(1) : goto(state.at + 1, performance.now());
    case "prev": return whole ? gotoBurst(-1) : goto(state.at - 1, performance.now());
    case "zoom": return toggleNative();
    case "compare": return togglePin();
    case "label": return applyChange(invoke("toggle_label", { index }), dom.label);
    case "reject": {
      const change = applyChange(invoke("toggle_reject", { index }), dom.reject);
      goto(state.at + 1);
      return change;
    }
    case "unassign":
      closeModal();
      return applyChange(invoke("assign", { index, collection: null }), dom.collection);
  }
  const star = action?.match(/^star(\d)$/);
  if (star) return applyChange(invoke("set_star", { index, stars: Number(star[1]) }), dom.rating);
}

document.addEventListener("keydown", (e) => {
  if (state.busy) { e.preventDefault(); return; }
  const key = normalise(e.key);
  if (state.modal) {
    if (state.capturing) {
      e.preventDefault();
      if (e.key === "Escape") return stopCapture();
      if (["Shift", "Control", "Alt", "Meta"].includes(e.key)) return;
      return void captureKey(e.key).catch(reportError);
    }
    if (e.key === "Escape") { e.preventDefault(); closeModal(); return; }
    if (e.key === "Tab") {
      const focusable = [...dom[state.modal].querySelectorAll('button:not([disabled]), input:not([disabled]), summary, [tabindex="0"]')].filter(n => n.getClientRects().length);
      const first = focusable[0], last = focusable[focusable.length - 1];
      if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
      else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
      return;
    }
    // Let native controls activate themselves; Enter on Cancel must never apply.
    if ((e.key === "Enter" || e.key === " ") && e.target.closest("button, summary, input")) return;
    MODAL_KEYS[state.modal]?.(e);
    return;
  }
  if (e.target.closest("input, textarea, select, [contenteditable]")) return;
  if (window.gridUI?.active && window.gridUI.keydown(e)) return;
  if (e.ctrlKey || e.metaKey) {
    const action = key === "z" ? (e.shiftKey ? "redo" : "undo") : e.key === "Enter" ? "apply" : null;
    if (action) { e.preventDefault(); executeAction(action).catch(reportError); }
    return;
  }
  if ((e.key === "Enter" || e.key === " ") && e.target.closest("button, summary")) return;
  if (e.key === "Escape") {
    e.preventDefault();
    el("more-actions").open = false;
    resetView();
    return;
  }
  const action = state.byKey.get(key);
  if (!action) return;
  e.preventDefault();
  executeAction(action, e[BURST_MODIFIER]).catch(reportError);
});

// ----------------------------------------------------------------- mouse

dom.stage.addEventListener("contextmenu", (e) => e.preventDefault());

dom.stage.addEventListener(
  "wheel",
  (e) => {
    if (state.modal || state.busy || window.gridUI?.active || !state.visible.length) return;
    e.preventDefault();
    const box = pane("current");
    const step = Math.exp(-e.deltaY / 400);
    setScale(
      state.view.scale * step,
      e.clientX - dom.stage.getBoundingClientRect().left - box.cx,
      e.clientY - dom.stage.getBoundingClientRect().top - box.h / 2,
    );
  },
  { passive: false },
);

let drag = null;
dom.stage.addEventListener("pointerdown", (e) => {
  if (state.modal || state.busy || window.gridUI?.active || !state.visible.length || e.button !== 0 || e.target.closest("button, .card")) return;
  if (!stillOnly()) return;
  drag = { x: e.clientX, y: e.clientY, moved: false };
  dom.stage.setPointerCapture(e.pointerId);
  document.body.classList.add("dragging");
});

dom.stage.addEventListener("pointermove", (e) => {
  if (!drag) return;
  state.view.x += e.clientX - drag.x;
  state.view.y += e.clientY - drag.y;
  drag.x = e.clientX;
  drag.y = e.clientY;
  drag.moved = true;
  clampPan();
  placeShown();
});

for (const end of ["pointerup", "pointercancel"]) {
  dom.stage.addEventListener(end, (e) => {
    if (!drag) return;
    // A click that never moved is a double-click candidate, not a pan.
    drag = null;
    dom.stage.releasePointerCapture?.(e.pointerId);
    document.body.classList.remove("dragging");
  });
}

dom.stage.addEventListener("dblclick", (e) => {
  if (state.modal || state.busy || window.gridUI?.active || !state.visible.length || e.target.closest("button, .card")) return;
  e.preventDefault();
  toggleNative();
});

// ----------------------------------------------------------------- setup

let noticeTimer;
let noticeKind = null;
let disabledBeforeBusy = new Map();

function notifyUser(message, error = false) {
  clearTimeout(noticeTimer);
  noticeKind = error ? "error" : "message";
  el("notice").textContent = message;
  el("notice").hidden = false;
  noticeTimer = setTimeout(() => { el("notice").hidden = true; }, error ? 12000 : 5000);
}

function reportError(error) {
  notifyUser(String(error), true);
}

function setBusy(busy, message) {
  state.busy = busy;
  document.body.setAttribute("aria-busy", String(busy));
  if (busy) {
    disabledBeforeBusy = new Map();
    document.querySelectorAll("button, input").forEach(button => {
      disabledBeforeBusy.set(button, button.disabled);
      button.disabled = true;
    });
    if (message) {
      clearTimeout(noticeTimer);
      noticeKind = "busy";
      el("notice").textContent = message;
      el("notice").hidden = false;
    }
  } else {
    for (const [button, disabled] of disabledBeforeBusy) button.disabled = disabled;
    disabledBeforeBusy.clear();
    if (noticeKind === "busy") el("notice").hidden = true;
  }
  renderStatus();
  if (!busy && state.modal && !dom[state.modal].contains(document.activeElement)) {
    dom[state.modal].querySelector("button:not([disabled]), input:not([disabled])")?.focus();
  }
}

function updateCollectionCounts() {
  state.collectionCounts = state.collections.map(() => 0);
  for (const assigned of state.assigned) {
    if (assigned !== null && assigned !== undefined) state.collectionCounts[assigned]++;
  }
  renderCollections();
}

function renderCollections() {
  const list = el("sidebar-list");
  const focusedCollection = document.activeElement.closest?.("#sidebar-list [data-collection]")?.dataset.collection;
  list.replaceChildren();
  if (!state.collections.length) {
    const message = document.createElement("p");
    message.className = "muted";
    message.textContent = t("collections.emptyHint");
    list.append(message);
  }
  state.collections.forEach((name, index) => {
    const button = document.createElement("button");
    button.className = "collection-row";
    button.dataset.collection = index;
    button.dataset.photo = "";
    button.dataset.command = `collection${index + 1}`;
    button.disabled = !state.visible.length || state.busy;
    button.setAttribute("aria-pressed", String(state.assigned[current()] === index));
    button.title = `${name} · Alt + clique atribui a rajada inteira`;
    button.style.setProperty("--chip", chipColour(name));
    const key = document.createElement("kbd");
    key.textContent = showKey(pickerKey(index));
    const label = document.createElement("span");
    label.className = "collection-name";
    label.textContent = name;
    const count = document.createElement("small");
    count.textContent = state.collectionCounts[index] ?? 0;
    button.append(key, label, count);
    list.append(button);
  });
  if (focusedCollection !== undefined && !state.modal) {
    list.querySelector(`[data-collection="${focusedCollection}"]`)?.focus();
  }
}

function toggleCollections() {
  const open = el("collections-sidebar").hidden;
  el("collections-sidebar").hidden = !open;
  document.body.classList.toggle("collections-open", open);
  document.querySelectorAll('[data-command="collections"]').forEach(button => button.setAttribute("aria-expanded", String(open)));
  renderStatus();
  if (!open) document.querySelector('nav [data-command="collections"]').focus();
}

/// The panel stays open while you navigate, so a burst can be compared field by
/// field. Closing it stops the fetching too.
function toggleExif(force) {
  const open = force ?? el("exif-sidebar").hidden;
  el("exif-sidebar").hidden = !open;
  document.body.classList.toggle("exif-open", open);
  document.querySelectorAll('[data-command="exif"]').forEach(b => b.setAttribute("aria-expanded", String(open)));
  if (open) renderExif();
  else document.querySelector('nav [data-command="exif"]')?.focus();
}

/// Only the fields the camera actually recorded get a row.
///
/// An adapted manual lens reports no aperture, no focal length and no name, so
/// a fixed list would be half blank on every frame. Four true rows read better
/// than ten with six dashes.
async function renderExif() {
  if (el("exif-sidebar").hidden || !state.photos.length) return;
  const index = current();
  const exif = await invoke("exif", { index }).catch(() => null);
  if (el("exif-sidebar").hidden) return;

  const rows = [];
  const photo = state.photos[index];
  if (exif) {
    if (exif.shutter) rows.push([t("exif.shutter"), exif.shutter]);
    if (exif.aperture != null) rows.push([t("exif.aperture"), `f/${exif.aperture.toFixed(1)}`]);
    if (exif.iso != null) rows.push([t("exif.iso"), String(exif.iso)]);
    if (exif.focalMm != null) rows.push([t("exif.focal"), `${Math.round(exif.focalMm)} mm`]);
    if (exif.exposureBias != null && exif.exposureBias !== 0) {
      rows.push([t("exif.bias"), `${exif.exposureBias > 0 ? "+" : ""}${exif.exposureBias.toFixed(1)} EV`]);
    }
    if (exif.width && exif.height) rows.push([t("exif.size"), `${exif.width} × ${exif.height}`]);
    if (exif.camera) rows.push([t("exif.camera"), exif.camera]);
    if (exif.lens) rows.push([t("exif.lens"), exif.lens]);
  }
  if (photo?.captured != null) rows.push([t("exif.captured"), fullTimestamp(photo.captured)]);

  const body = el("exif-body");
  body.replaceChildren();
  for (const [label, value] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = label;
    const dd = document.createElement("dd");
    dd.textContent = value;
    body.append(dt, dd);
  }
  el("exif-empty").hidden = rows.length > 0;
}

function renderShortcutHints() {
  document.querySelectorAll("[data-command]").forEach(button => {
    const action = button.dataset.command;
    const key = state.keymap[action];
    if (typeof key !== "string") return;
    button.title = `${button.getAttribute("aria-label") ?? button.textContent.trim()} · ${showKey(key)}`;
    button.setAttribute("aria-keyshortcuts", key === " " ? "Space" : key);
  });
  document.querySelectorAll("[data-key]").forEach(cap => cap.textContent = showKey(state.keymap[cap.dataset.key]));
}

function initInterface() {
  dom.stage.tabIndex = -1;
  document.querySelectorAll(".panel").forEach(panel => {
    const heading = panel.querySelector("h2");
    heading.id ||= `${panel.id}-title`;
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    panel.setAttribute("aria-labelledby", heading.id);
    panel.tabIndex = -1;
    const close = document.createElement("button");
    close.className = "panel-close";
    close.dataset.close = "";
    close.setAttribute("aria-label", t("common.closeDialog"));
    close.textContent = "×";
    panel.append(close);
    const cancel = document.createElement("button");
    cancel.dataset.close = "";
    cancel.textContent = ["apply", "newCollection", "filter", "move"].includes(panel.id) ? "Cancelar" : "Fechar";
    const actions = document.createElement("div");
    actions.className = "dialog-actions";
    actions.append(cancel);
    panel.append(actions);
  });
  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-command], [data-close]");
    if (!button || button.disabled || state.busy) return;
    if (button.hasAttribute("data-close")) return closeModal();
    el("more-actions").open = false;
    executeAction(button.dataset.command, event.altKey).catch(reportError);
  });
  document.querySelector(".brand").addEventListener("click", event => {
    event.preventDefault();
    if (!state.busy) executeAction("help").catch(reportError);
  });
  renderStatus();
}

dom.applyGo.addEventListener("click", () => confirmApply().catch(reportError));
dom.recoveryGo.addEventListener("click", () => restoreSession().catch(reportError));
el("recovery-discard").addEventListener("click", () => discardRecovery().catch(reportError));
el("collection-create").addEventListener("click", confirmNewCollection);

dom.keymapReset.addEventListener("click", async () => {
  stopCapture();
  dom.keymapError.hidden = true;
  adoptKeymap(await invoke("reset_keymap"));
});

dom.collectionName.addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    e.preventDefault();
    confirmNewCollection();
  }
});

new ResizeObserver(() => {
  clampPan();
  placeShown();
  renderStatus();
}).observe(dom.stage);

setInterval(() => invoke("checkpoint").catch(() => {}), CHECKPOINT_MS);

initInterface();
loadSettings().catch(reportError);
