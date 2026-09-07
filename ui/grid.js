"use strict";

// Selection is independent of the viewer cursor. Only the virtual window owns
// canvases; at most two JPEGs are decoded at once and then released.
(() => {
  const selected = new Set();
  const cards = new Map();
  const scroll = el("grid-scroll"), content = el("grid-content");
  const ROW = 246, GAP = 12;
  let active = false, focused = null, anchor = null, columns = 1;
  let generation = 0, loading = 0, queue = [], scheduled = false;
  const actions = new Set(["selectAll", "clearSelection", "applyXmp", "applyOrganization", "clearRating", "clearGreen", "label", "reject", "unassign", "prev", "next", "zoom", "compare", ...[1,2,3,4,5].map(n => `star${n}`)]);
  // Collection bindings are positional and share the same batch path.
  for (let i = 1; i <= 100; i++) actions.add(`collection${i}`);

  function status() {
    el("batch-clear").hidden = !active;
    document.querySelector('[data-command="photo"]').setAttribute("aria-pressed", String(!active));
    document.querySelector('[data-command="grid"]').setAttribute("aria-pressed", String(active));
    if (!active) return;
    el("selection-count").textContent = `${selected.size} selecionada${selected.size === 1 ? "" : "s"}`;
    el("grid-total").textContent = `${state.visible.length} fotos no filtro`;
    const indices = [...selected];
    const mark = indices.length ? state.marks[indices[0]] : null;
    const mixed = indices.some(i => state.marks[i].rating !== mark.rating || state.marks[i].label !== mark.label || state.assigned[i] !== state.assigned[indices[0]]);
    dom.name.textContent = selected.size ? `${selected.size} fotos selecionadas` : "Selecione fotos para editar em lote";
    dom.burst.textContent = mixed ? "Valores variados" : "As ações abaixo valem para a seleção";
    dom.rating.querySelectorAll("button").forEach((button, i) => {
      const same = !!indices.length && indices.every(index => state.marks[index].rating === i + 1);
      button.classList.toggle("on", !!indices.length && indices.every(index => state.marks[index].rating > i));
      button.setAttribute("aria-pressed", String(same));
    });
    dom.label.setAttribute("aria-pressed", String(!!indices.length && indices.every(i => state.marks[i].label === "Green")));
    dom.reject.setAttribute("aria-pressed", String(!!indices.length && indices.every(i => state.marks[i].rating === -1)));
    dom.reject.textContent = "Rejeitar";
    dom.collection.hidden = true;
    document.querySelectorAll("[data-photo], [data-selection]").forEach(b => b.disabled = !selected.size || state.busy);
    document.querySelectorAll("#sidebar-list [data-collection]").forEach(b => b.setAttribute("aria-pressed", String(!!indices.length && indices.every(i => state.assigned[i] === Number(b.dataset.collection)))));
    document.querySelector(".sidebar-tip").textContent = "Na grade, clique atribui todas as selecionadas.";
    for (const [index, card] of cards) updateCard(index, card);
  }

  function updateCard(index, card) {
    const chosen = selected.has(index), mark = state.marks[index];
    card.setAttribute("aria-selected", String(chosen));
    card.tabIndex = index === focused ? 0 : -1;
    card.querySelector(".selection-tick").textContent = chosen ? "✓" : "";
    card.querySelector(".grid-marks").textContent = `${mark.rating === -1 ? "Rejeitada" : mark.rating > 0 ? "★".repeat(mark.rating) : "Sem nota"}${mark.label ? " · Verde" : ""}`;
    const collection = state.assigned[index];
    const chip = card.querySelector(".grid-collection");
    chip.textContent = collection == null ? "Sem coleção" : state.collections[collection];
    chip.style.setProperty("--chip", collection == null ? "var(--muted)" : chipColour(state.collections[collection]));
    card.querySelector(".pending-dot").textContent = state.pendingXmp[index] || collection != null ? "●" : "";
  }

  function schedule() {
    if (scheduled || !active) return;
    scheduled = true;
    requestAnimationFrame(() => { scheduled = false; render(); });
  }

  function render() {
    if (!active) return;
    const width = Math.max(200, scroll.clientWidth - 28);
    columns = Math.max(1, Math.floor((width + GAP) / 220));
    const cardWidth = (width - GAP * (columns - 1)) / columns;
    const rows = Math.ceil(state.visible.length / columns);
    content.style.height = `${rows * ROW}px`;
    scroll.setAttribute("aria-rowcount", rows);
    scroll.setAttribute("aria-colcount", columns);
    const first = Math.max(0, Math.floor(scroll.scrollTop / ROW) - 2);
    const last = Math.min(rows, Math.ceil((scroll.scrollTop + scroll.clientHeight) / ROW) + 2);
    const wanted = new Set(state.visible.slice(first * columns, last * columns));
    for (const [index, card] of cards) if (!wanted.has(index)) { card.remove(); cards.delete(index); }
    for (const row of content.querySelectorAll('[role="row"]')) {
      if (Number(row.dataset.row) < first || Number(row.dataset.row) >= last) row.remove();
    }
    for (let r = first; r < last; r++) {
      let row = content.querySelector(`[data-row="${r}"]`);
      if (!row) {
        row = document.createElement("div"); row.dataset.row = r; row.setAttribute("role", "row"); row.setAttribute("aria-rowindex", r + 1);
        row.style.cssText = `position:absolute;top:${r * ROW}px;width:100%;height:${ROW}px`;
        content.append(row);
      }
      for (let col = 0; col < columns; col++) {
        const index = state.visible[r * columns + col];
        if (index === undefined) continue;
        let card = cards.get(index);
        if (!card) {
          card = document.createElement("div"); card.className = "grid-card"; card.dataset.index = index; card.setAttribute("role", "gridcell");
          card.setAttribute("aria-label", state.photos[index].name);
          card.innerHTML = '<span class="selection-tick" aria-hidden="true"></span><div class="grid-image"></div><div class="grid-caption"><span class="grid-name"></span><span class="grid-marks"></span><span class="grid-collection"></span></div>';
          const name = card.querySelector(".grid-name");
          const dot = document.createElement("span"); dot.className = "pending-dot"; dot.title = "Alterações pendentes";
          name.textContent = state.photos[index].name; name.append(dot);
          card.title = state.photos[index].name;
          card.addEventListener("click", event => {
            if (state.busy || event.detail > 1) return;
            select(index, event.shiftKey); card.focus();
          });
          card.addEventListener("dblclick", () => { if (!state.busy) openPhoto(index); });
          cards.set(index, card); queue.push({ index, card, generation });
        }
        card.style.cssText = `left:${col * (cardWidth + GAP)}px;width:${cardWidth}px;height:${ROW - GAP}px`;
        card.setAttribute("aria-colindex", col + 1);
        row.append(card); updateCard(index, card);
      }
    }
    queue = queue.filter(task => task.generation === generation && cards.get(task.index) === task.card);
    invoke("thumbnail_focus", { frames: [...wanted] }).catch(reportError);
    pump();
  }

  async function loadThumbnail(task) {
    const image = new Image();
    try {
      image.src = `${convertFileSrc(`thumb/${task.index}`, "zaru")}?v=${task.generation}`;
      try { await image.decode(); } catch {
        image.src = frameUrl(task.index); await image.decode();
      }
      if (task.generation !== generation || cards.get(task.index) !== task.card) return;
      const photo = state.photos[task.index], turned = photo.rotation % 180 !== 0;
      const factor = Math.min(1, 384 / Math.max(image.naturalWidth, image.naturalHeight));
      const w = image.naturalWidth * factor, h = image.naturalHeight * factor;
      const canvas = document.createElement("canvas");
      canvas.width = Math.round(turned ? h : w); canvas.height = Math.round(turned ? w : h);
      const ctx = canvas.getContext("2d");
      ctx.translate(canvas.width / 2, canvas.height / 2); ctx.rotate(photo.rotation * Math.PI / 180);
      if (photo.mirrored) ctx.scale(-1, 1);
      ctx.drawImage(image, -w / 2, -h / 2, w, h);
      task.card.querySelector(".grid-image").replaceChildren(canvas);
    } catch {
      if (task.generation === generation && cards.get(task.index) === task.card) {
        const error = document.createElement("span"); error.className = "thumbnail-error"; error.textContent = "Prévia indisponível · Enter abre a foto";
        task.card.querySelector(".grid-image").replaceChildren(error);
      }
    } finally { image.removeAttribute("src"); }
  }

  function pump() {
    while (loading < 2 && queue.length) {
      const task = queue.shift(); loading++;
      loadThumbnail(task).finally(() => { loading--; pump(); });
    }
  }

  function select(index, range) {
    focused = index;
    if (range && anchor !== null && state.visible.includes(anchor)) {
      const a = state.visible.indexOf(anchor), b = state.visible.indexOf(index);
      for (const i of state.visible.slice(Math.min(a,b), Math.max(a,b) + 1)) selected.add(i);
    } else {
      if (selected.has(index)) selected.delete(index); else selected.add(index);
      anchor = index;
    }
    status();
  }

  function openPhoto(index) {
    state.at = state.visible.indexOf(index);
    toggle(false);
  }

  function toggle(value = !active) {
    if (!state.photos.length || state.busy || value === active) return;
    active = value;
    document.body.classList.toggle("grid-mode", active);
    el("grid-view").hidden = !active;
    el("batch-clear").hidden = !active;
    if (active) {
      focused ??= current();
      for (const slot of [...state.slots, reference]) { slot.index = null; slot.el.removeAttribute("src"); slot.box.classList.remove("shown"); }
      invoke("focus", { frames: [] }).catch(reportError);
      schedule(); scroll.focus();
    } else {
      invalidate();
      invoke("thumbnail_focus", { frames: [] }).catch(reportError);
      document.querySelector(".sidebar-tip").textContent = "Alt + clique atribui a rajada inteira.";
      show(); fillRing(); dom.stage.focus();
    }
    renderStatus();
  }

  function invalidate() { generation++; queue = []; cards.clear(); content.replaceChildren(); if (active) schedule(); }
  function filterChanged() {
    const visible = new Set(state.visible);
    for (const index of selected) if (!visible.has(index)) selected.delete(index);
    if (!visible.has(focused)) focused = state.visible[0] ?? null;
    if (!visible.has(anchor)) anchor = null;
    invalidate();
  }

  async function edit(edit) {
    if (!selected.size || state.busy) return;
    const indices = [...selected]; setBusy(true);
    try {
      const changes = await invoke("edit_selection", { indices, edit });
      for (const change of changes) {
        state.marks[change.index] = change.mark; state.assigned[change.index] = change.collection;
        state.pendingXmp[change.index] = change.pendingXmp ?? true;
      }
      updateCollectionCounts(); render();
    } finally { setBusy(false); }
  }

  async function action(name) {
    if (name === "selectAll") { for (const i of state.visible) selected.add(i); status(); return; }
    if (name === "clearSelection") { selected.clear(); status(); return; }
    if (!selected.size) return;
    if (name === "applyXmp" || name === "applyOrganization") return openApply(name === "applyXmp" ? "xmp" : "organization", [...selected]);
    if (name.startsWith("star")) return edit({ kind: "rating", value: Number(name.slice(4)) });
    if (name.startsWith("collection")) return edit({ kind: "collection", value: Number(name.slice(10)) - 1 });
    if (name === "unassign") { closeModal(); return edit({ kind: "collection", value: null }); }
    if (name === "label" || name === "clearGreen") return edit({ kind: "green", value: name === "label" });
    if (name === "reject" || name === "clearRating") return edit({ kind: "rating", value: name === "reject" ? -1 : 0 });
  }

  function keydown(event) {
    if (state.busy) return false;
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") { event.preventDefault(); action("selectAll"); return true; }
    if (event.key === "Escape") { event.preventDefault(); selected.clear(); status(); return true; }
    if ((event.ctrlKey || event.metaKey) && event.key === " " && focused !== null) { event.preventDefault(); select(focused, false); return true; }
    if (event.target.closest("button, summary")) return false;
    if (event.key === "Enter" && !event.ctrlKey && !event.metaKey && focused !== null) { event.preventDefault(); openPhoto(focused); return true; }
    const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns }[event.key];
    if (!step || !state.visible.length) return false;
    event.preventDefault();
    const before = focused;
    const at = Math.max(0, Math.min(state.visible.length - 1, state.visible.indexOf(focused) + step));
    focused = state.visible[at];
    if (event.shiftKey) { anchor ??= before; select(focused, true); }
    const row = Math.floor(at / columns), top = row * ROW;
    if (top < scroll.scrollTop) scroll.scrollTop = top;
    else if (top + ROW > scroll.scrollTop + scroll.clientHeight) scroll.scrollTop = top + ROW - scroll.clientHeight;
    render(); cards.get(focused)?.focus(); status(); return true;
  }

  window.gridUI = { get active() { return active; }, actions, toggle, render, renderStatus: status, edit, action, keydown, invalidate, filterChanged,
    reset() { selected.clear(); anchor = focused = null; scroll.scrollTop = 0; invalidate(); },
  };
  scroll.addEventListener("scroll", schedule);
  new ResizeObserver(schedule).observe(scroll);
})();
