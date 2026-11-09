// jotbar frontend: plain JS, no build step.
// Two views: a list (capture box + timeline of notes) and an editor for one note.
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};

const state = {
  config: null,
  view: "list",
  notes: [],            // NoteSummary[] currently shown
  allTags: [],          // [tag, count][]
  filterTags: [],       // tags the list is filtered by
  captureTags: [],      // tags chosen for the note being captured
  sel: -1,              // keyboard-selected list row
  current: null,        // id open in the editor
  currentMeta: null,
  dirty: false,
  saveTimer: null,
  syncTimer: null,
};

// ---------------- time formatting ----------------

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dateFmt = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short" });
const yearFmt = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric" });
const fullFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" });

function when(ms) {
  const d = new Date(ms);
  const now = new Date();
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const day = 86400000;
  if (ms >= startOfToday) return timeFmt.format(d);
  if (ms >= startOfToday - day) return "Yesterday " + timeFmt.format(d);
  if (ms >= startOfToday - 6 * day) return dayFmt.format(d) + " " + timeFmt.format(d);
  if (d.getFullYear() === now.getFullYear()) return dateFmt.format(d);
  return yearFmt.format(d);
}

function ago(ms) {
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 10) return "just now";
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return when(ms);
}

// ---------------- tags ----------------

const TAG_RE = /(^|[\s(])#([\p{L}\p{N}_\-/]+)/gu;
function tagsOf(text) {
  const out = [];
  for (const m of text.matchAll(TAG_RE)) {
    const t = m[2].replace(/[-/]+$/, "").toLowerCase();
    if (/\p{L}/u.test(t) && !out.includes(t)) out.push(t);
  }
  return out;
}
const cleanTag = (t) => t.trim().replace(/^#+/, "").toLowerCase().replace(/[^\p{L}\p{N}_\-/]/gu, "");

function tagChip(tag, { count, on, onClick, onRemove } = {}) {
  const c = el(onClick ? "button" : "span", "tag" + (on ? " on" : ""));
  if (onClick) { c.type = "button"; c.onclick = onClick; }
  c.append("#" + tag);
  if (count != null) c.append(el("span", "n", count));
  if (onRemove) {
    const x = el("span", "x", "×");
    x.onclick = (e) => { e.stopPropagation(); onRemove(); };
    c.append(x);
  }
  return c;
}

// ---------------- list view ----------------

async function refreshList() {
  const res = await invoke("list_notes", { query: $("filter").value, tags: state.filterTags });
  state.notes = res.notes;
  state.allTags = res.tags;
  // drop filter tags that no longer exist
  state.filterTags = state.filterTags.filter((t) => res.tags.some(([x]) => x === t));
  state.sel = Math.min(state.sel, state.notes.length - 1);
  renderList();
  renderTagFilter();
}

function renderTagFilter() {
  $("tag-filter").replaceChildren(
    ...state.allTags.map(([t, n]) =>
      tagChip(t, {
        count: n,
        on: state.filterTags.includes(t),
        onClick: () => toggleFilterTag(t),
      })
    )
  );
}

function toggleFilterTag(t) {
  state.filterTags = state.filterTags.includes(t)
    ? state.filterTags.filter((x) => x !== t)
    : [...state.filterTags, t];
  refreshList();
}

function highlight(text, q) {
  const frag = document.createDocumentFragment();
  q = q.trim();
  const i = q ? text.toLowerCase().indexOf(q.toLowerCase()) : -1;
  if (i < 0) { frag.append(text); return frag; }
  const m = el("mark", null, text.slice(i, i + q.length));
  frag.append(text.slice(0, i), m, text.slice(i + q.length));
  return frag;
}

function renderList() {
  const q = $("filter").value;
  const ul = $("notes");
  ul.replaceChildren(
    ...state.notes.map((n, i) => {
      const li = el("li", i === state.sel ? "sel" : "");
      li.tabIndex = -1;
      li.role = "option";
      li.dataset.id = n.id;
      const title = el("span", "title");
      title.append(highlight(n.title, q));
      const t = el("time", null, when(n.created_at));
      t.dateTime = new Date(n.created_at).toISOString();
      t.title = "Created " + fullFmt.format(new Date(n.created_at)) +
        (n.updated_at - n.created_at > 60000 ? "\nEdited " + fullFmt.format(new Date(n.updated_at)) : "");
      li.append(title, t);
      if (n.preview) {
        const p = el("div", "preview");
        p.append(highlight(n.preview, q));
        li.append(p);
      }
      if (n.tags.length) {
        const chips = el("div", "chips");
        chips.append(...n.tags.map((tag) => tagChip(tag, {
          on: state.filterTags.includes(tag),
          onClick: (e) => { e.stopPropagation(); toggleFilterTag(tag); },
        })));
        li.append(chips);
      }
      li.onclick = () => openNote(n.id);
      return li;
    })
  );
  const empty = $("empty");
  empty.hidden = state.notes.length > 0;
  empty.textContent = q || state.filterTags.length
    ? "No notes match."
    : "No notes yet.\nType above and press Ctrl+Enter.";
  ul.querySelector(".sel")?.scrollIntoView({ block: "nearest" });
}

function moveSel(d) {
  if (!state.notes.length) return;
  state.sel = Math.max(0, Math.min(state.notes.length - 1, state.sel + d));
  renderList();
  $("notes").querySelector(".sel")?.focus();
}

$("filter").addEventListener("input", () => { state.sel = -1; refreshList(); });
$("filter").addEventListener("keydown", (e) => {
  if (e.key === "ArrowDown") { e.preventDefault(); state.sel = -1; moveSel(1); }
  if (e.key === "Enter" && state.notes.length) openNote(state.notes[Math.max(0, state.sel)].id);
});
$("notes").addEventListener("keydown", (e) => {
  const n = state.notes[state.sel];
  if (e.key === "ArrowDown") { e.preventDefault(); moveSel(1); }
  else if (e.key === "ArrowUp") {
    e.preventDefault();
    if (state.sel <= 0) { state.sel = -1; renderList(); $("filter").focus(); } else moveSel(-1);
  } else if (e.key === "Enter" && n) openNote(n.id);
  else if ((e.key === "Delete" || e.key === "Backspace") && n) askDelete(n.id, n.title);
});

// ---------------- capture ----------------

const captureText = $("capture-text");
const tagEntry = $("tag-entry");

function autoGrow() {
  captureText.style.height = "auto";
  captureText.style.height = Math.min(captureText.scrollHeight, 220) + "px";
}

function renderCaptureTags() {
  const box = $("capture-tags");
  box.querySelectorAll(".tag").forEach((c) => c.remove());
  const chips = state.captureTags.map((t) =>
    tagChip(t, { onRemove: () => { state.captureTags = state.captureTags.filter((x) => x !== t); renderCaptureTags(); } })
  );
  for (const c of chips) box.insertBefore(c, tagEntry);
  renderSuggestions();
}

function addCaptureTag(raw) {
  const t = cleanTag(raw);
  if (t && /\p{L}/u.test(t) && !state.captureTags.includes(t)) state.captureTags.push(t);
  tagEntry.value = "";
  renderCaptureTags();
}

function renderSuggestions() {
  const box = $("tag-suggest");
  const q = cleanTag(tagEntry.value);
  const inline = tagsOf(captureText.value);
  const show = document.activeElement === tagEntry;
  const sugg = state.allTags
    .map(([t]) => t)
    .filter((t) => !state.captureTags.includes(t) && !inline.includes(t) && t.startsWith(q))
    .slice(0, 12);
  box.hidden = !show || !sugg.length;
  box.replaceChildren(...sugg.map((t) => {
    const c = tagChip(t, { onClick: () => { addCaptureTag(t); tagEntry.focus(); } });
    c.onmousedown = (e) => e.preventDefault(); // keep focus in the tag field
    return c;
  }));
}

tagEntry.addEventListener("input", () => {
  if (/[\s,]/.test(tagEntry.value)) {
    tagEntry.value.split(/[\s,]+/).forEach((p, i, a) => (i < a.length - 1 ? addCaptureTag(p) : (tagEntry.value = p)));
  }
  renderSuggestions();
});
tagEntry.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) return; // handled by the form
  if (e.key === "Enter") {
    e.preventDefault();
    if (tagEntry.value.trim()) addCaptureTag(tagEntry.value);
    else submitCapture();
  } else if (e.key === "Backspace" && !tagEntry.value && state.captureTags.length) {
    state.captureTags.pop();
    renderCaptureTags();
  } else if (e.key === "Tab" && !e.shiftKey && tagEntry.value.trim()) {
    e.preventDefault();
    const first = $("tag-suggest").querySelector(".tag");
    addCaptureTag(first && !$("tag-suggest").hidden ? first.textContent.slice(1) : tagEntry.value);
  }
});
tagEntry.addEventListener("focus", renderSuggestions);
tagEntry.addEventListener("blur", () => {
  if (tagEntry.value.trim()) addCaptureTag(tagEntry.value);
  $("tag-suggest").hidden = true;
});
$("capture-tags").onclick = () => tagEntry.focus();

async function submitCapture() {
  if (tagEntry.value.trim()) addCaptureTag(tagEntry.value);
  const text = captureText.value;
  if (!text.trim() && !state.captureTags.length) return captureText.focus();
  try {
    await invoke("create_note", { content: text, tags: state.captureTags });
  } catch (e) {
    return setSaveState(String(e));
  }
  captureText.value = "";
  captureHist.reset();
  state.captureTags = [];
  captureMagic.clear();
  renderCaptureTags();
  autoGrow();
  $("filter").value = "";
  state.sel = -1;
  await refreshList();
  setSaveState("added " + timeFmt.format(new Date()));
  scheduleSync();
  captureText.focus();
}

$("capture").addEventListener("submit", (e) => { e.preventDefault(); submitCapture(); });
captureText.addEventListener("input", () => { autoGrow(); renderSuggestions(); });
// Ctrl+Enter adds the note from anywhere in the capture box.
$("capture").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submitCapture(); }
});

// ---------------- edit view ----------------

const editor = $("editor");

async function openNote(id) {
  state.current = id;
  state.currentMeta = state.notes.find((n) => n.id === id) ?? null;
  editor.value = await invoke("read_note", { id });
  editorHist.reset();
  state.dirty = false;
  showView("edit");
  renderEditHead();
  editor.focus();
  editor.setSelectionRange(editor.value.length, editor.value.length);
  editor.scrollTop = editor.scrollHeight;
}

function renderEditHead() {
  const m = state.currentMeta;
  $("edit-meta").textContent = m
    ? when(m.created_at) + (m.updated_at - m.created_at > 60000 ? " · edited " + ago(m.updated_at) : "")
    : "";
  $("edit-meta").title = m ? "Created " + fullFmt.format(new Date(m.created_at)) : "";
  $("edit-tags").replaceChildren(...tagsOf(editor.value).map((t) => tagChip(t)));
}

async function backToList() {
  await flushSave();
  // A note emptied in the editor is removed rather than left blank.
  if (state.current && !editor.value.trim()) {
    await invoke("delete_note", { id: state.current });
    scheduleSync();
  }
  state.current = null;
  showView("list");
  await refreshList();
  const i = state.sel;
  if (i >= 0 && state.notes[i]) $("notes").querySelector(".sel")?.focus();
  else captureText.focus();
}

function showView(v) {
  state.view = v;
  $("list-view").hidden = v !== "list";
  $("edit-view").hidden = v !== "edit";
  captureMagic.clear();
  editorMagic.clear();
}

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
    if (state.currentMeta) state.currentMeta.updated_at = meta.updated_at;
    setSaveState("saved");
    scheduleSync();
  } catch (e) {
    state.dirty = true;
    setSaveState("save failed: " + e);
  }
}

function setSaveState(t) { $("save-state").textContent = t; }

editor.addEventListener("input", () => { scheduleSave(); renderEditHead(); });
editor.addEventListener("blur", flushSave);
$("back").onclick = backToList;
$("delete-note").onclick = () => askDelete(state.current, state.currentMeta?.title);

// ---------------- delete ----------------

let pendingDelete = null;

function askDelete(id, title) {
  if (!id) return;
  pendingDelete = id;
  $("confirm-text").textContent = `Delete “${title || "this note"}”?`;
  showOverlay("confirm");
  $("confirm-yes").focus();
}

$("confirm-yes").onclick = async () => {
  const id = pendingDelete;
  hideOverlay("confirm");
  if (!id) return;
  clearTimeout(state.saveTimer);
  state.dirty = false;
  await invoke("delete_note", { id });
  scheduleSync();
  if (state.current === id) {
    state.current = null;
    showView("list");
  }
  await refreshList();
  captureText.focus();
};
$("confirm-no").onclick = () => { hideOverlay("confirm"); focusView(); };

// ---------------- undo / redo ----------------

/// Undo history for a textarea. WebKitGTK's built-in undo is unreliable
/// (and loses programmatic edits), so jotbar keeps its own: typing is
/// grouped into word-sized steps, and app edits are recorded explicitly.
function makeHistory(ta, onChange) {
  let undo = [];
  let redo = [];
  let before = null;
  let lastFamily = "";
  let last = 0;
  const snap = () => ({ v: ta.value, s: ta.selectionStart, e: ta.selectionEnd });
  const push = (st) => {
    if (undo.length && undo[undo.length - 1].v === st.v) return;
    undo.push(st);
    if (undo.length > 500) undo.shift();
  };
  function apply(st) {
    ta.value = st.v;
    ta.setSelectionRange(st.s, st.e);
    lastFamily = "";
    onChange();
  }
  function doUndo() {
    if (!undo.length) return;
    redo.push(snap());
    apply(undo.pop());
  }
  function doRedo() {
    if (!redo.length) return;
    undo.push(snap());
    apply(redo.pop());
  }

  ta.addEventListener("beforeinput", (e) => {
    if (e.inputType === "historyUndo") { e.preventDefault(); return doUndo(); }
    if (e.inputType === "historyRedo") { e.preventDefault(); return doRedo(); }
    before = snap();
  });
  ta.addEventListener("input", (e) => {
    const t = e.inputType || "other";
    const family = t === "insertText" ? "ins" : t.startsWith("delete") ? "del" : t;
    const now = Date.now();
    const grouped =
      (family === "ins" || family === "del") &&
      family === lastFamily &&
      now - last < 1500 &&
      !(e.data && /\s/.test(e.data));
    if (!grouped && before) push(before);
    redo = [];
    before = null;
    lastFamily = family;
    last = now;
  });
  ta.addEventListener("keydown", (e) => {
    if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === "z" && !e.shiftKey) { e.preventDefault(); doUndo(); }
    else if ((k === "z" && e.shiftKey) || k === "y") { e.preventDefault(); doRedo(); }
  });

  return {
    /// Call before the app itself changes the text.
    checkpoint() {
      push(snap());
      redo = [];
      lastFamily = "";
    },
    reset() {
      undo = [];
      redo = [];
      lastFamily = "";
    },
  };
}

// ---------------- magic mode (ghost-text completion) ----------------

function magicOn() { return !!state.config?.magic?.enabled && !!state.config?.magic?.model; }

/// Attach inline AI completion to a textarea whose sibling .ghost mirrors it.
function makeMagic(ta, hist, onAccept) {
  const ghost = ta.parentElement.querySelector(".ghost");
  let suggestion = "";
  let timer = null;
  let seq = 0;

  function clear() {
    suggestion = "";
    seq++;
    clearTimeout(timer);
    ghost.replaceChildren();
  }
  function render() {
    const pos = ta.selectionStart;
    const g = el("span", "g", suggestion);
    ghost.replaceChildren(ta.value.slice(0, pos), g, ta.value.slice(pos));
    ghost.scrollTop = ta.scrollTop;
  }
  async function request() {
    const pos = ta.selectionStart;
    if (pos !== ta.selectionEnd || document.activeElement !== ta) return;
    const before = ta.value.slice(0, pos);
    const after = ta.value.slice(pos);
    if (!before.trim() || (after && !after.startsWith("\n"))) return;
    const my = ++seq;
    $("magic-toggle").textContent = "✦ thinking…";
    try {
      let s = await invoke("complete", { before, after });
      if (my !== seq || ta.selectionStart !== pos || ta.value.slice(0, pos) !== before) return;
      if (s && /[\p{L}\p{N}.,;:!?]$/u.test(before) && /^[\p{L}\p{N}]/u.test(s)) s = " " + s;
      suggestion = s;
      if (s) render();
    } catch (e) {
      $("magic-toggle").title = "Magic mode error: " + e;
    } finally {
      renderMagicChip();
    }
  }
  function accept(wordOnly) {
    let s = suggestion;
    if (wordOnly) s = s.match(/^\s*\S+/)?.[0] ?? s;
    const rest = suggestion.slice(s.length);
    hist.checkpoint();
    ta.setRangeText(s, ta.selectionStart, ta.selectionEnd, "end");
    onAccept();
    if (rest) { suggestion = rest; render(); } else clear();
  }

  ta.addEventListener("input", () => {
    clear();
    if (magicOn()) timer = setTimeout(request, 650);
  });
  ta.addEventListener("scroll", () => (ghost.scrollTop = ta.scrollTop));
  ta.addEventListener("click", clear);
  ta.addEventListener("blur", clear);
  ta.addEventListener("keydown", (e) => {
    if (!suggestion) return;
    if (e.key === "Tab" && !e.shiftKey) { e.preventDefault(); accept(false); }
    else if (e.key === "ArrowRight" && e.ctrlKey) { e.preventDefault(); accept(true); }
    else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); clear(); }
    else if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End"].includes(e.key)) clear();
  });
  return { clear };
}

const captureHist = makeHistory(captureText, () => { captureMagic.clear(); autoGrow(); renderSuggestions(); });
const editorHist = makeHistory(editor, () => { editorMagic.clear(); scheduleSave(); renderEditHead(); });
const captureMagic = makeMagic(captureText, captureHist, autoGrow);
const editorMagic = makeMagic(editor, editorHist, () => { scheduleSave(); renderEditHead(); });
// Registered after magic mode so Tab accepts a suggestion before indenting.
editor.addEventListener("keydown", (e) => {
  if (e.key === "Tab" && !e.ctrlKey && !e.defaultPrevented) {
    e.preventDefault();
    editorHist.checkpoint();
    editor.setRangeText("  ", editor.selectionStart, editor.selectionEnd, "end");
    scheduleSave();
  }
});

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
  captureMagic.clear();
  editorMagic.clear();
}

// ---------------- overlays & global keys ----------------

const OVERLAYS = ["confirm", "settings"];
function showOverlay(id) {
  for (const o of OVERLAYS) $(o).hidden = o !== id;
}
function hideOverlay(id) { $(id).hidden = true; }
function focusView() { (state.view === "edit" ? editor : captureText).focus(); }

document.addEventListener("keydown", (e) => {
  const mod = e.ctrlKey || e.metaKey;
  const k = e.key.toLowerCase();
  if (e.key === "Escape") {
    for (const id of OVERLAYS) {
      if (!$(id).hidden) { hideOverlay(id); focusView(); return; }
    }
    if (state.view === "edit") return void backToList();
    if ($("filter").value || state.filterTags.length) {
      $("filter").value = "";
      state.filterTags = [];
      refreshList();
      return captureText.focus();
    }
    invoke("hide_window");
    return;
  }
  if (e.altKey && e.key === "ArrowLeft" && state.view === "edit") return void backToList();
  if (!mod) return;
  if (k === "n" || k === "t") {
    e.preventDefault();
    (state.view === "edit" ? backToList() : Promise.resolve()).then(() => captureText.focus());
  } else if (k === "f" || k === "k") {
    e.preventDefault();
    (state.view === "edit" ? backToList() : Promise.resolve()).then(() => { $("filter").focus(); $("filter").select(); });
  } else if (k === "w" && state.view === "edit") { e.preventDefault(); askDelete(state.current, state.currentMeta?.title); }
  else if (k === ",") { e.preventDefault(); openSettings(); }
  else if (k === "s") { e.preventDefault(); flushSave().then(syncNow); }
  else if (k === "m" && e.shiftKey) { e.preventDefault(); toggleMagic(); }
  else if (k === "=" || k === "+" || k === "-" || k === "0") {
    e.preventDefault();
    const cfg = structuredClone(state.config);
    cfg.font_size = k === "0" ? 15 : Math.min(32, Math.max(10, cfg.font_size + (k === "-" ? -1 : 1)));
    applyConfig(cfg);
  }
});

$("open-settings").onclick = openSettings;
$("close-settings").onclick = () => { hideOverlay("settings"); focusView(); };

// ---------------- sync status ----------------

function scheduleSync() {
  if (!state.config?.sync?.enabled) return;
  clearTimeout(state.syncTimer);
  state.syncTimer = setTimeout(syncNow, 3000);
}

async function syncNow() {
  if (!state.config?.sync?.enabled) return openSettings();
  try { await invoke("sync_now"); } catch (_) { /* reported via sync-status */ }
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
setInterval(() => {
  renderSync();
  if (state.view === "list") renderList(); // keep "Yesterday"/times fresh
}, 30000);

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
  $("interval-field").hidden = $("test-row").hidden = v === "off";
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
  showOverlay("settings");
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
      const b = el("button", null, m);
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
    hideOverlay("settings");
    focusView();
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
  autoGrow();
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
  r.style.setProperty("--muted", `rgba(${rgb},0.72)`);
  r.style.setProperty("--faint", `rgba(${rgb},0.5)`);
  r.style.setProperty("--line", `rgba(${rgb},0.14)`);
  r.style.setProperty("--accent", wal.colors.color4);
}

// ---------------- backend events ----------------

listen("notes-changed", async () => {
  if (state.view === "list") return refreshList();
  const res = await invoke("list_notes", {});
  state.allTags = res.tags;
  const meta = res.notes.find((n) => n.id === state.current);
  if (!meta) { state.current = null; showView("list"); return refreshList(); }
  state.currentMeta = meta;
  if (!state.dirty) {
    const text = await invoke("read_note", { id: state.current });
    if (text !== editor.value) {
      const [a, b, top] = [editor.selectionStart, editor.selectionEnd, editor.scrollTop];
      editorHist.checkpoint(); // a synced change can be undone like any edit
      editor.value = text;
      editor.setSelectionRange(Math.min(a, text.length), Math.min(b, text.length));
      editor.scrollTop = top;
    }
  }
  renderEditHead();
});
listen("sync-status", (e) => { syncStatus = e.payload; renderSync(); });
listen("open-settings", openSettings);
listen("shown", () => {
  if (OVERLAYS.every((o) => $(o).hidden)) focusView();
  if (state.view === "list") refreshList();
});
window.addEventListener("blur", flushSave);

// ---------------- boot ----------------

(async function boot() {
  state.config = (await invoke("get_config")).config;
  applyLook();
  applyTheme();
  renderMagicChip();
  syncStatus = await invoke("sync_status");
  renderSync();
  await refreshList();
  captureText.focus();
})();
