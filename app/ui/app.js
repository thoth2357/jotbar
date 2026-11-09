// jotbar frontend: plain JS, no build step.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

const editor = $("editor");
const ghost = $("ghost");
const tabsEl = $("tabs");

const state = {
  notes: [],          // NoteMeta[] in tab order
  current: null,      // id of the open note
  dirty: false,       // editor has unsaved text
  saveTimer: null,
  syncTimer: null,
  config: null,       // Config from the backend
  suggestion: "",     // magic-mode ghost text
  magicTimer: null,
  magicSeq: 0,
};

// ---------------- notes & tabs ----------------

async function loadNotes() {
  state.notes = await invoke("list_notes");
  if (!state.notes.some((n) => n.id === state.current)) {
    state.current = state.notes[0]?.id ?? null;
    await openNote(state.current, { force: true });
  }
  renderTabs();
}

function renderTabs() {
  tabsEl.replaceChildren(
    ...state.notes.map((n, i) => {
      const b = document.createElement("button");
      b.className = "tab" + (n.id === state.current ? " active" : "");
      b.role = "tab";
      b.draggable = true;
      b.dataset.id = n.id;
      b.title = n.title;
      if (i < 9) {
        const num = document.createElement("span");
        num.className = "num";
        num.textContent = i + 1;
        b.append(num);
      }
      b.append(n.title);
      b.onclick = () => openNote(n.id);
      b.onauxclick = (e) => e.button === 1 && askDelete(n.id);
      b.ondragstart = (e) => e.dataTransfer.setData("text/plain", n.id);
      b.ondragover = (e) => { e.preventDefault(); b.classList.add("drop-before"); };
      b.ondragleave = () => b.classList.remove("drop-before");
      b.ondrop = (e) => {
        e.preventDefault();
        b.classList.remove("drop-before");
        moveTab(e.dataTransfer.getData("text/plain"), n.id);
      };
      return b;
    })
  );
  tabsEl.querySelector(".active")?.scrollIntoView({ inline: "nearest" });
}

async function moveTab(id, beforeId) {
  if (id === beforeId) return;
  const ids = state.notes.map((n) => n.id).filter((x) => x !== id);
  ids.splice(ids.indexOf(beforeId), 0, id);
  await invoke("reorder_notes", { ids });
  await loadNotes();
}

async function openNote(id, { force = false, select = null } = {}) {
  if (!id) return;
  if (id === state.current && !force) return editor.focus();
  await flushSave();
  state.current = id;
  editor.value = await invoke("read_note", { id });
  clearGhost();
  renderTabs();
  editor.focus();
  if (select) {
    editor.setSelectionRange(select[0], select[1]);
    scrollToCaret();
  } else {
    editor.setSelectionRange(editor.value.length, editor.value.length);
    editor.scrollTop = editor.scrollHeight;
  }
}

async function newNote() {
  await flushSave();
  const meta = await invoke("create_note", { after: state.current });
  state.current = meta.id;
  await loadNotes();
  await openNote(meta.id, { force: true });
}

function cycleTab(dir) {
  const i = state.notes.findIndex((n) => n.id === state.current);
  const n = state.notes[(i + dir + state.notes.length) % state.notes.length];
  if (n) openNote(n.id);
}

// ---------------- saving ----------------

function scheduleSave() {
  state.dirty = true;
  setSaveState("editing…");
  clearTimeout(state.saveTimer);
  state.saveTimer = setTimeout(flushSave, 400);
}

async function flushSave() {
  clearTimeout(state.saveTimer);
  if (!state.dirty || !state.current) return;
  state.dirty = false;
  try {
    const meta = await invoke("save_note", { id: state.current, content: editor.value });
    const n = state.notes.find((x) => x.id === meta.id);
    if (n && n.title !== meta.title) {
      n.title = meta.title;
      renderTabs();
    }
    setSaveState("saved");
    scheduleSync();
  } catch (e) {
    state.dirty = true;
    setSaveState("save failed: " + e);
  }
}

function setSaveState(t) { $("save-state").textContent = t; }

// ---------------- delete ----------------

let pendingDelete = null;

async function askDelete(id) {
  await flushSave();
  const n = state.notes.find((x) => x.id === id);
  const text = await invoke("read_note", { id });
  if (!text.trim()) return doDelete(id); // empty: no need to ask
  pendingDelete = id;
  $("confirm-text").textContent = `Delete “${n?.title ?? "note"}”?`;
  show("confirm");
  $("confirm-yes").focus();
}

async function doDelete(id) {
  hide("confirm");
  const i = state.notes.findIndex((n) => n.id === id);
  await invoke("delete_note", { id });
  if (id === state.current) state.current = null;
  state.notes = await invoke("list_notes");
  const next = state.notes[Math.min(i, state.notes.length - 1)];
  state.current = null;
  await openNote(next.id, { force: true });
}

$("confirm-yes").onclick = () => pendingDelete && doDelete(pendingDelete);
$("confirm-no").onclick = () => { hide("confirm"); editor.focus(); };

// ---------------- search (all notes) ----------------

let hits = [];
let sel = 0;

function openSearch() {
  show("search");
  const q = $("search-input");
  q.value = editor.value.slice(editor.selectionStart, editor.selectionEnd).split("\n")[0];
  q.focus();
  q.select();
  runSearch();
}

async function runSearch() {
  const q = $("search-input").value;
  hits = q.trim() ? await invoke("search_notes", { query: q }) : [];
  sel = 0;
  renderHits(q);
}

function renderHits(q) {
  const ul = $("search-results");
  if (!q.trim()) return ul.replaceChildren();
  if (!hits.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No matches";
    return ul.replaceChildren(li);
  }
  ul.replaceChildren(
    ...hits.map((h, i) => {
      const li = document.createElement("li");
      li.className = i === sel ? "sel" : "";
      const t = document.createElement("div");
      t.className = "t";
      t.textContent = h.title;
      const s = document.createElement("div");
      s.className = "s";
      const idx = h.snippet.toLowerCase().indexOf(q.trim().toLowerCase());
      if (idx >= 0) {
        const m = document.createElement("mark");
        m.textContent = h.snippet.slice(idx, idx + q.trim().length);
        s.append(h.snippet.slice(0, idx), m, h.snippet.slice(idx + q.trim().length));
      } else s.textContent = h.snippet;
      li.append(t, s);
      li.onclick = () => pickHit(i);
      return li;
    })
  );
  ul.querySelector(".sel")?.scrollIntoView({ block: "nearest" });
}

async function pickHit(i) {
  const h = hits[i];
  if (!h) return;
  const len = $("search-input").value.trim().length;
  hide("search");
  // offsets from the backend are in characters; JS strings are UTF-16
  const text = await invoke("read_note", { id: h.id });
  const start = [...text].slice(0, h.offset).join("").length;
  await openNote(h.id, { force: true, select: [start, start + len] });
}

$("search-input").oninput = runSearch;
$("search-input").onkeydown = (e) => {
  if (e.key === "ArrowDown") { sel = Math.min(sel + 1, hits.length - 1); renderHits($("search-input").value); e.preventDefault(); }
  if (e.key === "ArrowUp") { sel = Math.max(sel - 1, 0); renderHits($("search-input").value); e.preventDefault(); }
  if (e.key === "Enter") pickHit(sel);
};

function scrollToCaret() {
  // Measure caret position with the mirror div, then scroll the textarea to it.
  const before = document.createElement("span");
  const mark = document.createElement("span");
  mark.textContent = "|";
  before.textContent = editor.value.slice(0, editor.selectionStart);
  ghost.replaceChildren(before, mark);
  editor.scrollTop = Math.max(0, mark.offsetTop - editor.clientHeight / 3);
  clearGhost();
}

// ---------------- magic mode (ghost-text completion) ----------------

function magicOn() { return !!state.config?.magic?.enabled && !!state.config?.magic?.model; }

function clearGhost() {
  state.suggestion = "";
  state.magicSeq++;
  ghost.replaceChildren();
}

function renderGhost() {
  const pos = editor.selectionStart;
  const g = document.createElement("span");
  g.className = "g";
  g.textContent = state.suggestion;
  ghost.replaceChildren(editor.value.slice(0, pos), g, editor.value.slice(pos));
  ghost.scrollTop = editor.scrollTop;
}

function scheduleMagic() {
  clearTimeout(state.magicTimer);
  if (!magicOn()) return;
  state.magicTimer = setTimeout(requestMagic, 650);
}

async function requestMagic() {
  const pos = editor.selectionStart;
  if (pos !== editor.selectionEnd || document.activeElement !== editor) return;
  const before = editor.value.slice(0, pos);
  const after = editor.value.slice(pos);
  // Only complete at the end of a line, after something was typed.
  if (!before.trim() || (after && !after.startsWith("\n"))) return;
  const seq = ++state.magicSeq;
  $("magic-toggle").textContent = "✦ thinking…";
  try {
    let s = await invoke("complete", { before, after });
    if (seq !== state.magicSeq || editor.selectionStart !== pos || editor.value.slice(0, pos) !== before) return;
    // Add a space if the model forgot one between words.
    if (s && /\S$/.test(before) && /^[A-Za-z0-9]/.test(s) && /[A-Za-z0-9.,;:!?]$/.test(before)) s = " " + s;
    state.suggestion = s;
    if (s) renderGhost();
  } catch (e) {
    $("magic-toggle").title = "Magic mode error: " + e;
  } finally {
    renderMagicChip();
  }
}

function acceptGhost(wordOnly = false) {
  let s = state.suggestion;
  if (wordOnly) s = s.match(/^\s*\S+/)?.[0] ?? s;
  const rest = state.suggestion.slice(s.length);
  editor.setRangeText(s, editor.selectionStart, editor.selectionEnd, "end");
  scheduleSave();
  if (rest) {
    state.suggestion = rest;
    renderGhost();
  } else clearGhost();
}

function renderMagicChip() {
  const c = $("magic-toggle");
  c.textContent = "✦ magic";
  c.classList.toggle("on", magicOn());
  c.hidden = !state.config?.magic?.model;
}

$("magic-toggle").onclick = toggleMagic;
async function toggleMagic() {
  const cfg = structuredClone(state.config);
  if (!cfg.magic.model) return openSettings();
  cfg.magic.enabled = !cfg.magic.enabled;
  await applyConfig(cfg);
  if (!cfg.magic.enabled) clearGhost();
}

// ---------------- editor events ----------------

editor.addEventListener("input", () => {
  clearGhost();
  scheduleSave();
  scheduleMagic();
});
editor.addEventListener("scroll", () => (ghost.scrollTop = editor.scrollTop));
editor.addEventListener("click", clearGhost);
editor.addEventListener("blur", () => { clearTimeout(state.magicTimer); flushSave(); });
editor.addEventListener("keydown", (e) => {
  if (state.suggestion) {
    if (e.key === "Tab" && !e.shiftKey) { e.preventDefault(); return acceptGhost(); }
    if (e.key === "ArrowRight" && e.ctrlKey) { e.preventDefault(); return acceptGhost(true); }
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); return clearGhost(); }
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) clearGhost();
  }
  if (e.key === "Tab" && !e.ctrlKey) {
    e.preventDefault();
    editor.setRangeText("  ", editor.selectionStart, editor.selectionEnd, "end");
    scheduleSave();
  }
});

// ---------------- global keys ----------------

document.addEventListener("keydown", (e) => {
  const mod = e.ctrlKey || e.metaKey;
  const k = e.key.toLowerCase();
  if (e.key === "Escape") {
    for (const id of ["confirm", "search", "settings"]) {
      if (!$(id).hidden) { hide(id); editor.focus(); return; }
    }
    flushSave().then(() => invoke("hide_window"));
    return;
  }
  if (!mod) return;
  if (k === "t" || k === "n") { e.preventDefault(); newNote(); }
  else if (k === "w") { e.preventDefault(); askDelete(state.current); }
  else if (k === "f" || k === "k" || (k === "p" && !e.shiftKey)) { e.preventDefault(); openSearch(); }
  else if (k === ",") { e.preventDefault(); openSettings(); }
  else if (k === "s") { e.preventDefault(); flushSave().then(syncNow); }
  else if (k === "m" && e.shiftKey) { e.preventDefault(); toggleMagic(); }
  else if (e.key === "Tab" || e.key === "PageDown" || e.key === "PageUp") {
    e.preventDefault();
    cycleTab(e.shiftKey || e.key === "PageUp" ? -1 : 1);
  } else if (/^[1-9]$/.test(e.key)) {
    e.preventDefault();
    const n = e.key === "9" ? state.notes.at(-1) : state.notes[+e.key - 1];
    if (n) openNote(n.id);
  } else if (k === "=" || k === "+" || k === "-" || k === "0") {
    e.preventDefault();
    const cfg = structuredClone(state.config);
    cfg.font_size = k === "0" ? 15 : Math.min(32, Math.max(10, cfg.font_size + (k === "-" ? -1 : 1)));
    applyConfig(cfg);
  }
});

$("new-tab").onclick = newNote;
$("open-search").onclick = openSearch;
$("open-settings").onclick = openSettings;
$("close-settings").onclick = () => { hide("settings"); editor.focus(); };

function show(id) {
  for (const o of ["confirm", "search", "settings"]) if (o !== id) $(o).hidden = true;
  $(id).hidden = false;
}
function hide(id) { $(id).hidden = true; }

// ---------------- sync status ----------------

function scheduleSync() {
  if (!state.config?.sync?.enabled) return;
  clearTimeout(state.syncTimer);
  state.syncTimer = setTimeout(syncNow, 3000);
}

async function syncNow() {
  if (!state.config?.sync?.enabled) return openSettings();
  try { await invoke("sync_now"); } catch (_) { /* shown via sync-status event */ }
}

function ago(ms) {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 10) return "just now";
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  return `${Math.round(s / 3600)}h ago`;
}

let syncStatus = {};
function renderSync() {
  const c = $("sync-state");
  const on = state.config?.sync?.enabled && state.config?.sync?.target;
  c.classList.remove("on", "err");
  if (!on) { c.textContent = "local only"; c.title = "Notes stay on this machine. Click to set up sync."; return; }
  if (syncStatus.running) c.textContent = "⟳ syncing…";
  else if (syncStatus.error) { c.textContent = "⚠ sync error"; c.classList.add("err"); }
  else if (syncStatus.last_ok) { c.textContent = "✓ synced " + ago(syncStatus.last_ok); c.classList.add("on"); }
  else c.textContent = "⟳ waiting";
  c.title = syncStatus.error || "Sync now (Ctrl+S)";
}
$("sync-state").onclick = syncNow;
setInterval(renderSync, 15000);

// ---------------- settings ----------------

let draftTarget = "off";

function setSeg(id, v) {
  for (const b of $(id).children) b.classList.toggle("on", b.dataset.v === v);
}
for (const b of $("size-seg").children) b.onclick = () => setSeg("size-seg", b.dataset.v);
for (const b of $("target-seg").children) b.onclick = () => selectTarget(b.dataset.v);

function selectTarget(v) {
  draftTarget = v;
  setSeg("target-seg", v);
  for (const f of document.querySelectorAll(".target-fields")) f.hidden = f.dataset.for !== v;
  $("interval-field").hidden = $("test-target").parentElement.hidden = v === "off";
  $("target-result").textContent = "";
}

async function openSettings() {
  const view = await invoke("get_config");
  const c = view.config;
  state.config = c;
  setSeg("size-seg", c.size);
  $("font-size").value = c.font_size;
  $("hide-on-blur").checked = c.hide_on_blur;
  $("hotkey").value = c.hotkey;
  $("hotkey-note").textContent = view.hotkey_error
    ? "Global hotkey unavailable here (" + view.hotkey_error + "). On Wayland, bind `jotbar --toggle` in your compositor."
    : "";
  $("notes-dir").value = c.notes_dir ?? "";
  $("notes-dir-now").textContent = view.notes_dir;
  const t = c.sync.target;
  $("t-folder-path").value = t?.type === "folder" ? t.path : "";
  $("t-server-url").value = t?.type === "server" ? t.url : "";
  $("t-server-token").value = t?.type === "server" ? t.token : "";
  $("t-webdav-url").value = t?.type === "webdav" ? t.url : "";
  $("t-webdav-user").value = t?.type === "webdav" ? t.username : "";
  $("t-webdav-pass").value = t?.type === "webdav" ? t.password : "";
  $("sync-interval").value = c.sync.interval_secs;
  selectTarget(c.sync.enabled && t ? t.type : "off");
  $("magic-enabled").checked = c.magic.enabled;
  $("magic-endpoint").value = c.magic.endpoint;
  $("magic-model").value = c.magic.model;
  $("settings-error").textContent = "";
  show("settings");
  loadModels();
}

function readTarget() {
  switch (draftTarget) {
    case "folder": return { type: "folder", path: $("t-folder-path").value.trim() };
    case "server": return { type: "server", url: $("t-server-url").value.trim(), token: $("t-server-token").value };
    case "webdav": return {
      type: "webdav", url: $("t-webdav-url").value.trim(),
      username: $("t-webdav-user").value, password: $("t-webdav-pass").value,
    };
    default: return null;
  }
}

async function loadModels() {
  const chips = $("model-chips");
  chips.replaceChildren();
  try {
    const models = await invoke("magic_models", { endpoint: $("magic-endpoint").value || "http://127.0.0.1:11434" });
    chips.replaceChildren(...models.map((m) => {
      const b = document.createElement("button");
      b.textContent = m;
      b.onclick = () => { $("magic-model").value = m; };
      return b;
    }));
    if (!models.length) chips.textContent = "Ollama is running but has no models yet.";
  } catch (e) {
    chips.textContent = String(e);
  }
}
$("magic-endpoint").onchange = loadModels;

$("test-target").onclick = async () => {
  const r = $("target-result");
  r.className = "muted";
  r.textContent = "testing…";
  try {
    r.textContent = await invoke("test_target", { target: readTarget() });
  } catch (e) {
    r.className = "error";
    r.textContent = String(e);
  }
};

$("save-settings").onclick = async () => {
  const cfg = structuredClone(state.config);
  cfg.size = $("size-seg").querySelector(".on")?.dataset.v ?? "regular";
  cfg.font_size = Math.min(32, Math.max(10, +$("font-size").value || 15));
  cfg.hide_on_blur = $("hide-on-blur").checked;
  cfg.hotkey = $("hotkey").value.trim();
  cfg.notes_dir = $("notes-dir").value.trim() || null;
  const t = readTarget();
  cfg.sync.enabled = !!t;
  if (t) cfg.sync.target = t;
  cfg.sync.interval_secs = Math.max(10, +$("sync-interval").value || 60);
  cfg.magic.enabled = $("magic-enabled").checked;
  cfg.magic.endpoint = $("magic-endpoint").value.trim() || "http://127.0.0.1:11434";
  cfg.magic.model = $("magic-model").value.trim();
  try {
    await applyConfig(cfg);
    hide("settings");
    editor.focus();
    if (cfg.sync.enabled) syncNow();
  } catch (e) {
    $("settings-error").textContent = String(e);
  }
};

async function applyConfig(cfg) {
  const view = await invoke("set_config", { config: cfg });
  state.config = view.config;
  applyLook();
  renderMagicChip();
  renderSync();
}

function applyLook() {
  document.documentElement.style.setProperty("--text-size", state.config.font_size + "px");
}

// pywal integration: match the desktop palette when ~/.cache/wal/colors.json exists.
async function applyTheme() {
  const wal = await invoke("theme");
  if (!wal?.special) return;
  const r = document.documentElement;
  r.classList.add("wal");
  const fg = wal.special.foreground;
  const rgb = fg.match(/\w\w/g).map((h) => parseInt(h, 16)).join(",");
  r.style.setProperty("--bg", wal.special.background);
  r.style.setProperty("--bg2", wal.colors.color0);
  r.style.setProperty("--fg", fg);
  r.style.setProperty("--muted", `rgba(${rgb},0.68)`);
  r.style.setProperty("--faint", `rgba(${rgb},0.45)`);
  r.style.setProperty("--line", `rgba(${rgb},0.14)`);
  r.style.setProperty("--accent", wal.colors.color4);
}

// ---------------- backend events ----------------

listen("notes-changed", async () => {
  const list = await invoke("list_notes");
  state.notes = list;
  if (!list.some((n) => n.id === state.current)) {
    state.current = null;
    await openNote(list[0]?.id, { force: true });
  } else if (!state.dirty) {
    // Reload the open note in place, keeping the caret where it was.
    const text = await invoke("read_note", { id: state.current });
    if (text !== editor.value) {
      const [a, b, top] = [editor.selectionStart, editor.selectionEnd, editor.scrollTop];
      editor.value = text;
      editor.setSelectionRange(Math.min(a, text.length), Math.min(b, text.length));
      editor.scrollTop = top;
    }
  }
  renderTabs();
});
listen("sync-status", (e) => { syncStatus = e.payload; renderSync(); });
listen("open-settings", openSettings);
listen("shown", () => { if ($("settings").hidden && $("search").hidden) editor.focus(); });
window.addEventListener("blur", flushSave);

// ---------------- boot ----------------

(async function boot() {
  state.config = (await invoke("get_config")).config;
  applyLook();
  applyTheme();
  renderMagicChip();
  syncStatus = await invoke("sync_status");
  renderSync();
  await loadNotes();
  setSaveState("");
})();
