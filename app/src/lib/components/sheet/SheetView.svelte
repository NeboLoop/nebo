<script lang="ts">
  /**
   * SheetView — the spreadsheet grid of the Work panel's "sheet" mode.
   *
   * Renders the sheet engine's view model (lib/sheet/types.ts): tabs on top,
   * a virtualized grid (rows AND columns) with headers, gridlines, real
   * widths/heights, merges and freeze panes, a formula bar, keyboard/mouse
   * selection and a status bar. The engine owns every number: the grid shows
   * `display` as given, never formats or calculates. Only cells the engine
   * marks `editable` (unlocked in the file) take input; an edit goes to the
   * engine and the recalculated cells come back live.
   *
   * Layout: one native scroller sized to the whole sheet (real scrollbars,
   * trackpad momentum); inside it a sticky viewport holds four panes (frozen
   * corner, frozen rows, frozen columns, body) plus the header strips. Each
   * pane draws only the boxes in view; scrolling just moves the pane offsets.
   *
   * Styling: static styling is Tailwind/DaisyUI classes. Per-cell geometry and
   * the file's own colours are data, set as CSS custom properties through the
   * `vars` action (see lib/sheet/vars.ts for the documented exception).
   */
  import { onDestroy, onMount, tick } from 'svelte';
  import { t } from 'svelte-i18n';
  import { SheetApiError, editSheet, getSheet, saveSheet } from '$lib/sheet/client';
  import { sheetFromFile } from '$lib/sheet/sheetjs';
  import { beginSave, endSave, setPendingSave } from '$lib/sheet/pending';
  import { vars } from '$lib/sheet/vars';
  import {
    EditHistory,
    PAGE_ROWS,
    WorkbookModel,
    addr,
    cellKey,
    cellLook,
    colName,
    expandForMerges,
    fitDisplay,
    inputOf,
    paneBoxes,
    parseAddr,
    rangeName,
    rectOf,
    scrollToReveal,
    selectionStats,
    step,
    visibleWindow,
    type CellBox,
    type Rect,
    type SheetModel,
  } from '$lib/sheet/model';
  import type { SheetEdit, SheetViewModel } from '$lib/sheet/types';

  let {
    documentId,
    version,
    src,
    changedSheets = [],
    onsaved,
    agentId,
    sessionKey,
  }: {
    /** The work document; a `path:` id (a file opened by its path) renders read-only. */
    documentId?: string;
    version?: number;
    /** The file's URL — used only for the read-only fallback. */
    src: string;
    /** Sheets to mark with a dot (the employee changed them). */
    changedSheets?: string[];
    /** A save wrote a new version. */
    onsaved?: (version: number) => void;
    /** Who the chat's "Saved …" message is from, and the conversation it lands in. */
    agentId?: string;
    sessionKey?: string;
  } = $props();

  const ZOOMS = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
  const FONT_PX = 12;

  // ── Load ──────────────────────────────────────────────────────────────
  let book = $state.raw<WorkbookModel | null>(null);
  /** Bumped whenever the model mutates (Maps aren't reactive). */
  let rev = $state(0);
  let loading = $state(true);
  let loadError = $state('');
  let readOnly = $state(false);
  let session = '';
  // svelte-ignore state_referenced_locally
  let currentVersion = version;
  let activeName = $state('');
  const sheet = $derived<SheetModel | null>(book ? (book.sheet(activeName) ?? null) : null);
  const tabs = $derived(book ? (book.visibleSheets.length ? book.visibleSheets : book.sheets) : []);
  const anyEditable = $derived.by(() => {
    rev;
    return !!book && book.sheets.some((s) => [...s.cells.values()].some((c) => c.editable));
  });

  const engineDoc = $derived(!!documentId && !documentId.startsWith('path:'));

  async function load() {
    loading = true;
    loadError = '';
    try {
      let vm: SheetViewModel | null = null;
      if (engineDoc && documentId) {
        try {
          // A reload after a 409 asks for the latest version.
          vm = await getSheet(documentId, { version: stale ? undefined : version, rows: [1, PAGE_ROWS] });
        } catch (e) {
          // No engine for this file (an older backend, an unsupported file):
          // still show it, read-only, from the file itself.
          console.warn('[sheet] engine view unavailable, showing the file read-only', e);
        }
      }
      if (!vm) {
        vm = await sheetFromFile(src, documentId ?? '');
        readOnly = true;
      }
      const b = new WorkbookModel(vm);
      for (const s of b.sheets) {
        if (readOnly) s.fullyLoaded = true;
        else s.markPage(0, s.data.cells);
      }
      session = vm.session;
      currentVersion = vm.version || version;
      stale = false;
      editError = null;
      saveError = '';
      dirty = false;
      history.past = [];
      history.future = [];
      historyRev++;
      book = b;
      activeName = (b.visibleSheets[0] ?? b.sheets[0])?.name ?? '';
      loading = false;
      await tick();
      measure();
      focusGrid();
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  }

  // ── Geometry, viewport, virtualization ────────────────────────────────
  let scroller = $state<HTMLDivElement | null>(null);
  let vw = $state(0);
  let vh = $state(0);
  let scrollTop = $state(0);
  let scrollLeft = $state(0);
  let zoom = $state(1);

  const hdrW = $derived(sheet ? Math.round((String(sheet.rows.count).length * 7 + 14) * zoom) : 0);
  const hdrH = $derived(Math.round(20 * zoom));
  const fw = $derived.by(() => { rev; return sheet?.frozenWidth ?? 0; });
  const fh = $derived.by(() => { rev; return sheet?.frozenHeight ?? 0; });
  const bodyW = $derived(Math.max(0, vw - hdrW - fw));
  const bodyH = $derived(Math.max(0, vh - hdrH - fh));
  const totalW = $derived.by(() => { rev; return sheet ? hdrW + sheet.cols.total : 0; });
  const totalH = $derived.by(() => { rev; return sheet ? hdrH + sheet.rows.total : 0; });

  /** The visible window as a string, so panes rebuild only when it changes — not on every scrolled pixel. */
  const winKey = $derived.by(() => {
    rev;
    if (!sheet) return '';
    const w = visibleWindow(sheet, { scrollTop, scrollLeft, width: bodyW, height: bodyH }, 3);
    return `${w.r1},${w.r2},${w.c1},${w.c2}`;
  });

  interface Panes {
    corner: CellBox[];
    top: CellBox[];
    left: CellBox[];
    body: CellBox[];
    colHeads: { c: number; x: number; w: number; frozen: boolean }[];
    rowHeads: { r: number; y: number; h: number; frozen: boolean }[];
  }

  const panes = $derived.by<Panes>(() => {
    rev;
    const empty: Panes = { corner: [], top: [], left: [], body: [], colHeads: [], rowHeads: [] };
    if (!sheet || !winKey) return empty;
    const [r1, r2, c1, c2] = winKey.split(',').map(Number);
    const fr = sheet.freezeRows;
    const fc = sheet.freezeCols;
    const frozenRows: [number, number] = [1, fr];
    const frozenCols: [number, number] = [1, fc];
    const colHeads: Panes['colHeads'] = [];
    for (let c = 1; c <= fc; c++) colHeads.push({ c, x: sheet.cols.start(c), w: sheet.cols.size(c), frozen: true });
    for (let c = c1; c <= c2; c++) colHeads.push({ c, x: sheet.cols.start(c) - sheet.frozenWidth, w: sheet.cols.size(c), frozen: false });
    const rowHeads: Panes['rowHeads'] = [];
    for (let r = 1; r <= fr; r++) rowHeads.push({ r, y: sheet.rows.start(r), h: sheet.rows.size(r), frozen: true });
    for (let r = r1; r <= r2; r++) rowHeads.push({ r, y: sheet.rows.start(r) - sheet.frozenHeight, h: sheet.rows.size(r), frozen: false });
    return {
      corner: paneBoxes(sheet, frozenRows, frozenCols, 1, 1),
      top: paneBoxes(sheet, frozenRows, [c1, c2], 1, fc + 1),
      left: paneBoxes(sheet, [r1, r2], frozenCols, fr + 1, 1),
      body: paneBoxes(sheet, [r1, r2], [c1, c2], fr + 1, fc + 1),
      colHeads,
      rowHeads,
    };
  });

  function measure() {
    if (!scroller) return;
    vw = scroller.clientWidth;
    vh = scroller.clientHeight;
  }

  $effect(() => {
    if (!scroller) return;
    const ro = new ResizeObserver(measure);
    ro.observe(scroller);
    return () => ro.disconnect();
  });

  function onScroll() {
    if (!scroller) return;
    scrollTop = scroller.scrollTop;
    scrollLeft = scroller.scrollLeft;
  }

  function setZoom(z: number) {
    const next = Math.max(ZOOMS[0], Math.min(ZOOMS[ZOOMS.length - 1], z));
    if (!book || next === zoom) return;
    // Keep the top-left cell in place across the zoom.
    const r = sheet ? sheet.rows.indexAt(sheet.frozenHeight + scrollTop) : 1;
    const c = sheet ? sheet.cols.indexAt(sheet.frozenWidth + scrollLeft) : 1;
    zoom = next;
    for (const s of book.sheets) s.layout(next);
    rev++;
    void tick().then(() => {
      if (!scroller || !sheet) return;
      scroller.scrollTop = Math.max(0, sheet.rows.start(r) - sheet.frozenHeight);
      scroller.scrollLeft = Math.max(0, sheet.cols.start(c) - sheet.frozenWidth);
      onScroll();
    });
  }

  function zoomBy(dir: 1 | -1) {
    const i = ZOOMS.findIndex((z) => z >= zoom - 1e-6);
    setZoom(ZOOMS[Math.max(0, Math.min(ZOOMS.length - 1, (i < 0 ? 4 : i) + dir))]);
  }

  // ── Paging (big sheets) ───────────────────────────────────────────────
  const inflightPages = new Set<string>();

  $effect(() => {
    if (!winKey || !sheet || readOnly || !documentId) return;
    const [r1, r2] = winKey.split(',').map(Number);
    const s = sheet;
    for (const p of s.missingPages(r1, r2)) {
      const key = `${s.name}:${p}`;
      if (inflightPages.has(key)) continue;
      inflightPages.add(key);
      getSheet(documentId, { version: currentVersion, rows: [p * PAGE_ROWS + 1, (p + 1) * PAGE_ROWS], sheet: s.name, session })
        .then((vm) => {
          const page = vm.sheets.find((x) => x.name === s.name);
          if (!page) return;
          s.upsert(page.cells);
          s.markPage(p, page.cells);
          rev++;
        })
        .catch((e) => console.warn('[sheet] page load failed', s.name, p, e))
        .finally(() => inflightPages.delete(key));
    }
  });

  // ── Selection ─────────────────────────────────────────────────────────
  let anchor = $state({ r: 1, c: 1 });
  let focus = $state({ r: 1, c: 1 });
  let dragging = false;
  const memory = new Map<string, { anchor: { r: number; c: number }; focus: { r: number; c: number }; top: number; left: number }>();

  const sel = $derived<Rect>(sheet ? expandForMerges(sheet, rectOf(anchor, focus)) : rectOf(anchor, focus));
  const active = $derived.by(() => {
    rev;
    return sheet?.get(anchor.r, anchor.c);
  });
  const activeEditable = $derived.by(() => {
    rev;
    return !readOnly && !!sheet?.isEditable(anchor.r, anchor.c);
  });
  const nameBox = $derived.by(() => {
    if (!sheet) return '';
    const single = sel.r1 === sel.r2 && sel.c1 === sel.c2;
    const merged = sheet.mergeAt(anchor.r, anchor.c);
    if (single || (merged && merged.r1 === sel.r1 && merged.c1 === sel.c1 && merged.r2 === sel.r2 && merged.c2 === sel.c2)) {
      return book?.nameFor(sheet.name, anchor.r, anchor.c) ?? addr(anchor.r, anchor.c);
    }
    return rangeName(sel);
  });
  const stats = $derived.by(() => {
    rev;
    if (!sheet || (sel.r1 === sel.r2 && sel.c1 === sel.c2)) return null;
    const s = selectionStats(sheet, sel);
    return s.count ? s : null;
  });
  const numberFmt = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });

  function inSel(r: number, c: number): boolean {
    return r >= sel.r1 && r <= sel.r2 && c >= sel.c1 && c <= sel.c2;
  }

  function select(r: number, c: number, extend = false) {
    if (!sheet) return;
    const at = extend ? { r, c } : sheet.anchorOf(r, c);
    if (!extend) anchor = at;
    focus = at;
    hint = '';
    reveal(focus.r, focus.c);
  }

  function reveal(r: number, c: number) {
    if (!scroller || !sheet) return;
    const next = scrollToReveal(sheet, { scrollTop, scrollLeft, width: bodyW, height: bodyH }, r, c);
    if (next.scrollTop !== scrollTop) scroller.scrollTop = next.scrollTop;
    if (next.scrollLeft !== scrollLeft) scroller.scrollLeft = next.scrollLeft;
    onScroll();
  }

  async function switchSheet(name: string) {
    if (!sheet || name === activeName) return;
    await commitEdit();
    memory.set(activeName, { anchor, focus, top: scrollTop, left: scrollLeft });
    const m = memory.get(name);
    activeName = name;
    anchor = m?.anchor ?? { r: 1, c: 1 };
    focus = m?.focus ?? { r: 1, c: 1 };
    await tick();
    if (scroller) {
      scroller.scrollTop = m?.top ?? 0;
      scroller.scrollLeft = m?.left ?? 0;
    }
    onScroll();
    measure();
  }

  async function goto(sheetName: string, r: number, c: number) {
    if (sheetName !== activeName) await switchSheet(sheetName);
    await tick();
    select(r, c);
  }

  function focusGrid() {
    scroller?.focus({ preventScroll: true });
  }

  function cellFromEvent(e: Event): { r: number; c: number } | null {
    const el = (e.target as HTMLElement | null)?.closest?.('[data-r]') as HTMLElement | null;
    if (!el) return null;
    return { r: Number(el.dataset.r), c: Number(el.dataset.c) };
  }

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    const at = cellFromEvent(e);
    if (!at) return;
    if (editing && editing.r === at.r && editing.c === at.c) return;
    if (editing) void commitEdit();
    e.preventDefault();
    focusGrid();
    select(at.r, at.c, e.shiftKey);
    dragging = true;
    window.addEventListener('pointerup', () => (dragging = false), { once: true });
  }

  function onPointerOver(e: PointerEvent) {
    if (!dragging) return;
    const at = cellFromEvent(e);
    if (at && (at.r !== focus.r || at.c !== focus.c)) focus = at;
  }

  function onDblClick(e: MouseEvent) {
    const at = cellFromEvent(e);
    if (!at) return;
    select(at.r, at.c);
    startEdit('edit');
  }

  /** Header click: the whole column or row. */
  function selectColumn(c: number, extend: boolean) {
    if (!sheet) return;
    if (!extend) anchor = { r: 1, c };
    focus = { r: sheet.rows.count, c };
    focusGrid();
  }

  function selectRow(r: number, extend: boolean) {
    if (!sheet) return;
    if (!extend) anchor = { r, c: 1 };
    focus = { r, c: sheet.cols.count };
    focusGrid();
  }

  // ── Editing ───────────────────────────────────────────────────────────
  let editing = $state<{ sheet: string; r: number; c: number; mode: 'enter' | 'edit'; inBar: boolean } | null>(null);
  let editText = $state('');
  let busy = $state(0);
  let hint = $state('');
  let editError = $state<{ sheet: string; cell: string; message: string } | null>(null);
  const history = new EditHistory();
  let historyRev = $state(0);
  let queue: Promise<unknown> = Promise.resolve();

  /** Begin editing the active cell — only an editable one; locked cells never enter edit mode. */
  function startEdit(mode: 'enter' | 'edit', initial?: string, inBar = false): boolean {
    if (!sheet || readOnly || stale) return false;
    // A cell the view doesn't list (unlocked only by a row/column style) is
    // treated as locked until the engine reports it.
    if (!sheet.isEditable(anchor.r, anchor.c)) {
      hint = $t('sheet.locked');
      return false;
    }
    editing = { sheet: sheet.name, r: anchor.r, c: anchor.c, mode, inBar };
    editText = initial ?? inputOf(sheet.get(anchor.r, anchor.c));
    hint = '';
    return true;
  }

  function cancelEdit() {
    editing = null;
    editText = '';
    focusGrid();
  }

  async function commitEdit(move?: { dr: number; dc: number }) {
    const ed = editing;
    if (!ed || !book) return;
    const s = book.sheet(ed.sheet);
    editing = null;
    const before = inputOf(s?.get(ed.r, ed.c));
    const after = editText;
    if (move && sheet) {
      const to = step(sheet, anchor, move.dr, move.dc);
      select(to.r, to.c);
    }
    focusGrid();
    if (before === after) return;
    const cell = addr(ed.r, ed.c);
    const ok = await send([{ sheet: ed.sheet, cell, input: after }]);
    if (ok) {
      history.record({ sheet: ed.sheet, cell, before, after });
      historyRev++;
    }
  }

  /** POST edits (serialized), apply every changed cell, surface errors inline. */
  function send(edits: SheetEdit[]): Promise<boolean> {
    const run = async (): Promise<boolean> => {
      if (!book || !documentId) return false;
      busy++;
      try {
        const res = await editSheet(documentId, session, currentVersion ?? 0, edits);
        if (res.changed.length) {
          book.applyChanged(res.changed);
          rev++;
        }
        if (res.errors.length) {
          const err = res.errors[0];
          editError = { sheet: err.sheet, cell: err.cell, message: err.error };
          return false;
        }
        editError = null;
        markDirty();
        return true;
      } catch (e) {
        editError = { sheet: edits[0].sheet, cell: edits[0].cell, message: e instanceof Error ? e.message : String(e) };
        noteStale(e);
        return false;
      } finally {
        busy--;
      }
    };
    const p = queue.then(run, run);
    queue = p;
    return p;
  }

  async function undo() {
    const e = history.undo();
    historyRev++;
    if (!e) return;
    if (!(await send([e]))) {
      history.revertUndo();
      historyRev++;
    }
    const at = parseAddr(e.cell);
    if (at) await goto(e.sheet, at.r, at.c);
  }

  async function redo() {
    const e = history.redo();
    historyRev++;
    if (!e) return;
    if (!(await send([e]))) {
      history.revertRedo();
      historyRev++;
    }
    const at = parseAddr(e.cell);
    if (at) await goto(e.sheet, at.r, at.c);
  }

  const canUndo = $derived.by(() => { historyRev; return history.canUndo; });
  const canRedo = $derived.by(() => { historyRev; return history.canRedo; });

  // ── Save ──────────────────────────────────────────────────────────────
  // Every save cuts a version: idle autosave waits a minute so one editing
  // session makes one version (Save, Ctrl+S, panel close and Download still save at once).
  const AUTOSAVE_MS = 60_000;
  let dirty = $state(false);
  let saving = $state(false);
  let saveError = $state('');
  let justSaved = $state(false);
  let editSeq = 0;
  let autosaveTimer: ReturnType<typeof setTimeout> | undefined;
  let savePromise: Promise<void> | null = null;

  function markDirty() {
    dirty = true;
    justSaved = false;
    editSeq++;
    clearTimeout(autosaveTimer);
    autosaveTimer = setTimeout(() => void save(), AUTOSAVE_MS);
  }

  /** Write the session's edits as a new version. The version reaches the
   *  panel the way every new version does (the backend announces it); the
   *  panel knows it's ours (pending.ts) and keeps this viewer mounted. */
  function save(): Promise<void> {
    if (savePromise) return savePromise;
    if (!dirty || readOnly || !documentId) return Promise.resolve();
    clearTimeout(autosaveTimer);
    const doc = documentId;
    savePromise = (async () => {
      await queue;
      const seq = editSeq;
      saving = true;
      saveError = '';
      beginSave(doc);
      let saved: number | null = null;
      try {
        const res = await saveSheet(doc, session, { agentId, sessionKey });
        saved = res.version;
        currentVersion = saved;
        if (editSeq === seq) {
          dirty = false;
          justSaved = true;
        }
        onsaved?.(saved);
      } catch (e) {
        saveError = e instanceof Error ? e.message : String(e);
        noteStale(e);
      } finally {
        endSave(doc, saved);
        saving = false;
        savePromise = null;
      }
    })();
    return savePromise;
  }

  /** The document moved on (409) or the session expired (404): edits stop
   *  until a reload opens the latest version in a fresh session. */
  let stale = $state(false);
  function noteStale(e: unknown) {
    if (e instanceof SheetApiError && (e.status === 409 || e.status === 404)) {
      stale = true;
      clearTimeout(autosaveTimer);
    }
  }

  function reload() {
    currentVersion = undefined;
    void load();
  }

  /** Last chance (panel closed, page hidden): a keepalive request that outlives this view. */
  function saveOnLeave() {
    if (!dirty || readOnly || !documentId || savePromise) return;
    const doc = documentId;
    clearTimeout(autosaveTimer);
    dirty = false;
    beginSave(doc);
    saveSheet(doc, session, { agentId, sessionKey, keepalive: true })
      .then((r) => endSave(doc, r.version))
      .catch(() => endSave(doc, null));
  }

  $effect(() => {
    if (!documentId || readOnly) return;
    setPendingSave(documentId, dirty ? save : null);
  });

  onMount(() => {
    void load();
    window.addEventListener('pagehide', saveOnLeave);
  });

  onDestroy(() => {
    if (typeof window === 'undefined') return;
    window.removeEventListener('pagehide', saveOnLeave);
    clearTimeout(autosaveTimer);
    saveOnLeave();
    if (documentId) setPendingSave(documentId, null);
  });

  // ── Theme ─────────────────────────────────────────────────────────────
  /** Dark theme in effect (DaisyUI sets color-scheme per theme); file ink adapts to it. */
  let dark = $state(false);
  $effect(() => {
    const read = () => (dark = getComputedStyle(document.documentElement).colorScheme.includes('dark'));
    read();
    const mo = new MutationObserver(read);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] });
    const mq = matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', read);
    return () => {
      mo.disconnect();
      mq.removeEventListener('change', read);
    };
  });

  // ── Find ──────────────────────────────────────────────────────────────
  let findOpen = $state(false);
  let findQuery = $state('');
  let findIndex = $state(0);
  let findInput = $state<HTMLInputElement | null>(null);
  const findHits = $derived.by(() => {
    rev;
    return book && findOpen ? book.find(findQuery) : [];
  });

  async function openFind() {
    findOpen = true;
    await tick();
    findInput?.focus();
    findInput?.select();
  }

  function closeFind() {
    findOpen = false;
    focusGrid();
  }

  async function findNext(dir: 1 | -1) {
    if (!findHits.length) return;
    findIndex = (findIndex + dir + findHits.length) % findHits.length;
    const h = findHits[findIndex];
    await goto(h.sheet, h.r, h.c);
    findInput?.focus();
  }

  // ── Keyboard ──────────────────────────────────────────────────────────
  /** Shortcuts that work anywhere in the viewer. */
  function onRootKey(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey;
    if (!mod) return;
    const k = e.key.toLowerCase();
    if (k === 'f') {
      e.preventDefault();
      void openFind();
    } else if (k === '=' || k === '+') {
      e.preventDefault();
      zoomBy(1);
    } else if (k === '-') {
      e.preventDefault();
      zoomBy(-1);
    } else if (k === '0') {
      e.preventDefault();
      setZoom(1);
    } else if (k === 's' && !readOnly) {
      e.preventDefault();
      void commitEdit().then(save);
    }
  }

  function onGridKey(e: KeyboardEvent) {
    if (!sheet || editing) return;
    const mod = e.ctrlKey || e.metaKey;
    const k = e.key;
    const moves: Record<string, [number, number]> = {
      ArrowUp: [-1, 0],
      ArrowDown: [1, 0],
      ArrowLeft: [0, -1],
      ArrowRight: [0, 1],
    };
    if (moves[k]) {
      e.preventDefault();
      const [dr, dc] = moves[k];
      const from = e.shiftKey ? focus : anchor;
      const to = mod ? { r: dr ? (dr > 0 ? sheet.rows.count : 1) : from.r, c: dc ? (dc > 0 ? sheet.cols.count : 1) : from.c } : step(sheet, from, dr, dc);
      select(to.r, to.c, e.shiftKey);
      return;
    }
    if (k === 'Tab' || k === 'Enter') {
      e.preventDefault();
      const back = e.shiftKey ? -1 : 1;
      const to = k === 'Tab' ? step(sheet, anchor, 0, back) : step(sheet, anchor, back, 0);
      select(to.r, to.c);
      return;
    }
    if (mod && k.toLowerCase() === 'z') {
      e.preventDefault();
      void (e.shiftKey ? redo() : undo());
      return;
    }
    if (mod && k.toLowerCase() === 'y') {
      e.preventDefault();
      void redo();
      return;
    }
    if (mod && k.toLowerCase() === 'c') {
      e.preventDefault();
      copySelection();
      return;
    }
    if (mod && k.toLowerCase() === 'a') {
      e.preventDefault();
      anchor = { r: 1, c: 1 };
      focus = { r: sheet.rows.count, c: sheet.cols.count };
      return;
    }
    if (k === 'F2') {
      e.preventDefault();
      startEdit('edit');
      return;
    }
    if ((k === 'Delete' || k === 'Backspace') && !mod) {
      e.preventDefault();
      if (startEdit('enter', '')) void commitEdit();
      return;
    }
    if (k === 'Escape') {
      hint = '';
      return;
    }
    // Typing a character starts an edit that replaces the cell.
    if (k.length === 1 && !mod && !e.altKey) {
      e.preventDefault();
      startEdit('enter', k);
    }
  }

  function onEditorKey(e: KeyboardEvent) {
    if (!editing) return;
    const k = e.key;
    if (k === 'Enter') {
      e.preventDefault();
      void commitEdit({ dr: e.shiftKey ? -1 : 1, dc: 0 });
    } else if (k === 'Tab') {
      e.preventDefault();
      void commitEdit({ dr: 0, dc: e.shiftKey ? -1 : 1 });
    } else if (k === 'Escape') {
      e.preventDefault();
      cancelEdit();
    } else if (editing.mode === 'enter' && !editing.inBar && k.startsWith('Arrow')) {
      // Typed-over cells behave like Excel's Enter mode: arrows commit and move.
      e.preventDefault();
      const d = { ArrowUp: [-1, 0], ArrowDown: [1, 0], ArrowLeft: [0, -1], ArrowRight: [0, 1] }[k] ?? [0, 0];
      void commitEdit({ dr: d[0], dc: d[1] });
    }
    e.stopPropagation();
  }

  function onEditorBlur() {
    if (editing) void commitEdit();
  }

  function onBarFocus() {
    if (!editing && activeEditable) startEdit('edit', undefined, true);
  }

  function copySelection() {
    if (!sheet) return;
    const lines: string[] = [];
    for (let r = sel.r1; r <= sel.r2 && r - sel.r1 < 10000; r++) {
      const row: string[] = [];
      for (let c = sel.c1; c <= sel.c2; c++) row.push(sheet.get(r, c)?.display ?? '');
      lines.push(row.join('\t'));
    }
    void navigator.clipboard?.writeText(lines.join('\n'));
  }

  /** Name box: type an address or a defined name and press Enter to jump there. */
  function onNameKey(e: KeyboardEvent) {
    if (e.key !== 'Enter' || !book || !sheet) return;
    e.preventDefault();
    const text = (e.currentTarget as HTMLInputElement).value.trim();
    const ref = book.resolveName(sheet.name, text) ?? text;
    const at = parseAddr(ref);
    if (at) void goto(at.sheet && book.sheet(at.sheet) ? at.sheet : sheet.name, at.r, at.c).then(focusGrid);
  }

  /** Autofocus for the in-cell editor, caret at the end. */
  function autofocus(node: HTMLInputElement) {
    node.focus();
    const n = node.value.length;
    node.setSelectionRange(n, n);
  }

  // ── Text measure (for #### on numbers too wide for their column) ─────
  /** One canvas measures text in the grid's own font, per weight/style/size,
   *  cached; only drawn (visible) number cells ask. */
  let measureCtx: CanvasRenderingContext2D | null = null;
  const widths = new Map<string, number>();
  let fontFamily: string | undefined;
  function measurer(look: ReturnType<typeof cellLook>): (text: string) => number {
    const px = look.size ? Math.round(((look.size * 4) / 3) * zoom) : Math.round(FONT_PX * zoom);
    fontFamily ??= scroller ? getComputedStyle(scroller).fontFamily : undefined;
    const family = fontFamily ?? 'sans-serif';
    const font = `${look.italic ? 'italic ' : ''}${look.bold ? 600 : 400} ${px}px ${family}`;
    return (text) => {
      const key = `${font}|${text}`;
      let w = widths.get(key);
      if (w === undefined) {
        measureCtx ??= document.createElement('canvas').getContext('2d');
        if (!measureCtx) return 0;
        measureCtx.font = font;
        w = measureCtx.measureText(text).width;
        if (widths.size > 20000) widths.clear();
        widths.set(key, w);
      }
      return w;
    };
  }

  // ── Cell rendering ────────────────────────────────────────────────────
  const ALIGN = { left: 'justify-start text-left', center: 'justify-center text-center', right: 'justify-end text-right' };
  const VALIGN = { top: 'items-start', center: 'items-center', bottom: 'items-end' };

  function cellClass(b: CellBox, look: ReturnType<typeof cellLook>, editable: boolean, isActive: boolean, selected: boolean, hasError: boolean): string {
    const parts = [
      'absolute left-(--l) top-(--t) w-(--w) h-(--h) flex px-1 pb-px overflow-hidden border-r border-b border-base-content/10 cursor-cell select-none',
      ALIGN[look.align],
      VALIGN[look.valign],
      look.size ? 'text-(length:--cfz)' : '',
      look.wrap || b.merge ? 'whitespace-pre-wrap break-words' : 'whitespace-pre',
      look.fill ? 'bg-(--fill)' : editable ? 'bg-primary/5' : 'bg-base-100',
      look.ink ? 'text-(color:--ink)' : 'text-base-content',
    ];
    if (look.bold) parts.push('font-semibold');
    if (look.italic) parts.push('italic');
    if (look.underline) parts.push('underline');
    if (editable) parts.push('shadow-[inset_0_-2px_0_0_var(--color-primary)]');
    if (hasError) parts.push('ring-1 ring-inset ring-error');
    if (selected) parts.push("after:content-[''] after:absolute after:inset-0 after:bg-primary/15 after:pointer-events-none");
    if (isActive) parts.push('outline-2 -outline-offset-2 outline-primary z-10');
    return parts.join(' ');
  }

  function cellVars(b: CellBox, look: ReturnType<typeof cellLook>) {
    // A file font size (points → px) scales with zoom like everything else.
    const cfz = look.size ? Math.round(((look.size * 4) / 3) * zoom) : undefined;
    return { '--l': b.x, '--t': b.y, '--w': b.w, '--h': b.h, '--fill': look.fill, '--ink': look.ink, '--cfz': cfz };
  }

  function tabColor(c: string | null | undefined): string | undefined {
    if (!c) return undefined;
    return c.startsWith('#') ? c : `#${c.length === 8 ? c.slice(2) : c}`;
  }
</script>

{#snippet cells(boxes: CellBox[])}
  {#each boxes as b (b.key)}
    {@const look = cellLook(book?.styles[b.cell?.style ?? -1], b.cell, dark)}
    {@const editable = !readOnly && b.cell?.editable === true}
    {@const isEditing = editing && !editing.inBar && editing.sheet === activeName && editing.r === b.r && editing.c === b.c}
    {@const hasError = editError?.sheet === activeName && editError.cell === addr(b.r, b.c)}
    <div
      data-r={b.r}
      data-c={b.c}
      class={cellClass(b, look, editable, anchor.r === b.r && anchor.c === b.c, inSel(b.r, b.c) && !(anchor.r === b.r && anchor.c === b.c), hasError)}
      use:vars={cellVars(b, look)}
      title={hasError ? editError?.message : undefined}
    >
      {#if isEditing}
        <input
          class="absolute inset-0 w-full h-full px-1 bg-base-100 text-base-content border-none outline-2 -outline-offset-2 outline-primary"
          bind:value={editText}
          onkeydown={onEditorKey}
          onblur={onEditorBlur}
          use:autofocus
          aria-label={addr(b.r, b.c)}
        />
      {:else if editing?.inBar && editing.sheet === activeName && editing.r === b.r && editing.c === b.c}
        {editText}
      {:else}
        {fitDisplay(b.cell, b.w, measurer(look))}
      {/if}
    </div>
  {/each}
{/snippet}

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="h-full flex flex-col min-h-0 bg-base-100 text-base-content" onkeydown={onRootKey}>
  {#if loading}
    <div class="text-xs text-base-content/50 py-8 text-center">{$t('common.loading')}</div>
  {:else if loadError}
    <div class="text-xs text-error py-8 text-center" title={loadError}>{$t('chat.failedToRender')}</div>
  {:else if book && sheet}
    <!-- Sheet tabs, on top -->
    <div class="flex items-end gap-0.5 px-2 pt-1.5 bg-base-200 border-b border-base-300 overflow-x-auto scrollbar-slim shrink-0" role="tablist" aria-label={$t('sheet.sheets')}>
      {#each tabs as s (s.name)}
        {@const color = tabColor(s.data.tabColor)}
        <button
          role="tab"
          aria-selected={s.name === activeName}
          class="relative flex items-center gap-1.5 px-3 py-1.5 rounded-t-md text-xs whitespace-nowrap cursor-pointer border border-b-0 transition-colors {s.name === activeName ? 'bg-base-100 border-base-300 font-medium text-base-content' : 'bg-transparent border-transparent text-base-content/70 hover:text-base-content hover:bg-base-100/60'}"
          onclick={() => switchSheet(s.name)}
        >
          {s.name}
          {#if changedSheets.includes(s.name)}
            <span class="w-1.5 h-1.5 rounded-full bg-accent" aria-hidden="true"></span>
            <span class="sr-only">{$t('sheet.changedSheet')}</span>
          {/if}
          {#if color}
            <span class="absolute inset-x-2 bottom-0 h-0.5 rounded-full bg-(--tab)" use:vars={{ '--tab': color }} aria-hidden="true"></span>
          {/if}
        </button>
      {/each}
    </div>

    <!-- Formula bar -->
    <div class="flex items-center gap-1.5 px-2 py-1 border-b border-base-300 shrink-0">
      <input
        class="input input-xs w-24 shrink-0 font-mono text-xs"
        value={nameBox}
        onkeydown={onNameKey}
        aria-label={$t('sheet.cellName')}
      />
      <span class="text-xs italic text-base-content/50 shrink-0 px-1 font-mono" aria-hidden="true">fx</span>
      {#if editing?.inBar}
        <input
          class="input input-xs flex-1 min-w-0 font-mono text-xs"
          bind:value={editText}
          onkeydown={onEditorKey}
          onblur={onEditorBlur}
          use:autofocus
          aria-label={$t('sheet.formula')}
        />
      {:else}
        <input
          class="input input-xs flex-1 min-w-0 font-mono text-xs {activeEditable ? '' : 'text-base-content/70'}"
          value={editing ? editText : (active?.formula ?? active?.display ?? '')}
          readonly={!activeEditable}
          onfocus={onBarFocus}
          aria-label={$t('sheet.formula')}
        />
      {/if}
      {#if !readOnly}
        <button class="btn btn-ghost btn-xs btn-square shrink-0" disabled={!canUndo || busy > 0} onclick={undo} title={$t('sheet.undo')} aria-label={$t('sheet.undo')}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7v6h6"/><path d="M21 17a9 9 0 0 0-9-9 9 9 0 0 0-6 2.3L3 13"/></svg>
        </button>
        <button class="btn btn-ghost btn-xs btn-square shrink-0" disabled={!canRedo || busy > 0} onclick={redo} title={$t('sheet.redo')} aria-label={$t('sheet.redo')}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 7v6h-6"/><path d="M3 17a9 9 0 0 1 9-9 9 9 0 0 1 6 2.3l3 2.7"/></svg>
        </button>
        {#if saving}
          <span class="text-xs text-base-content/50 shrink-0 px-1">{$t('common.saving')}</span>
        {:else if dirty}
          <button class="btn btn-primary btn-xs shrink-0" onclick={() => commitEdit().then(save)}>{$t('common.save')}</button>
        {:else if justSaved}
          <span class="text-xs text-base-content/50 shrink-0 px-1">{$t('common.saved')}</span>
        {/if}
      {/if}
    </div>

    {#if findOpen}
      <div class="flex items-center gap-1.5 px-2 py-1 border-b border-base-300 bg-base-200/50 shrink-0">
        <input
          bind:this={findInput}
          class="input input-xs flex-1 min-w-0 text-xs"
          placeholder={$t('sheet.find')}
          aria-label={$t('sheet.find')}
          bind:value={findQuery}
          oninput={() => (findIndex = -1)}
          onkeydown={(e) => {
            if (e.key === 'Enter') { e.preventDefault(); void findNext(e.shiftKey ? -1 : 1); }
            else if (e.key === 'Escape') { e.preventDefault(); closeFind(); }
          }}
        />
        <span class="text-xs text-base-content/50 font-mono shrink-0">
          {#if findQuery.trim()}
            {findHits.length ? $t('sheet.findCount', { values: { current: Math.max(1, findIndex + 1), total: findHits.length } }) : $t('sheet.noMatches')}
          {/if}
        </span>
        <button class="btn btn-ghost btn-xs btn-square" disabled={!findHits.length} onclick={() => findNext(-1)} title={$t('common.previous')} aria-label={$t('common.previous')}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="18 15 12 9 6 15"/></svg>
        </button>
        <button class="btn btn-ghost btn-xs btn-square" disabled={!findHits.length} onclick={() => findNext(1)} title={$t('common.next')} aria-label={$t('common.next')}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <button class="btn btn-ghost btn-xs btn-square" onclick={closeFind} title={$t('common.close')} aria-label={$t('common.close')}>
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        </button>
      </div>
    {/if}

    <!-- The grid. Spreadsheets read left-to-right in every locale. -->
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      bind:this={scroller}
      dir="ltr"
      class="relative flex-1 min-h-0 overflow-auto outline-none bg-base-100"
      tabindex="0"
      role="grid"
      aria-label={sheet.name}
      aria-rowcount={sheet.rows.count}
      aria-colcount={sheet.cols.count}
      onscroll={onScroll}
      onkeydown={onGridKey}
      onpointerdown={onPointerDown}
      onpointerover={onPointerOver}
      ondblclick={onDblClick}
    >
      <div class="relative w-(--sw) h-(--sh)" use:vars={{ '--sw': totalW, '--sh': totalH }}>
        <div
          class="sticky top-0 left-0 overflow-hidden w-(--vw) h-(--vh) text-(length:--fz) leading-tight"
          use:vars={{ '--vw': vw, '--vh': vh, '--fz': Math.round(FONT_PX * zoom) }}
        >
          <!-- Body panes: frozen corner, frozen rows, frozen columns, scrolling body. -->
          <div class="absolute overflow-hidden left-(--l) top-(--t) w-(--w) h-(--h)" use:vars={{ '--l': hdrW + fw, '--t': hdrH + fh, '--w': bodyW, '--h': bodyH }}>
            <div class="absolute left-(--ox) top-(--oy)" use:vars={{ '--ox': -scrollLeft, '--oy': -scrollTop }}>
              {@render cells(panes.body)}
            </div>
          </div>
          {#if fh > 0}
            <div class="absolute overflow-hidden left-(--l) top-(--t) w-(--w) h-(--h) border-b border-base-content/30" use:vars={{ '--l': hdrW + fw, '--t': hdrH, '--w': bodyW, '--h': fh }}>
              <div class="absolute left-(--ox) top-0" use:vars={{ '--ox': -scrollLeft }}>
                {@render cells(panes.top)}
              </div>
            </div>
          {/if}
          {#if fw > 0}
            <div class="absolute overflow-hidden left-(--l) top-(--t) w-(--w) h-(--h) border-r border-base-content/30" use:vars={{ '--l': hdrW, '--t': hdrH + fh, '--w': fw, '--h': bodyH }}>
              <div class="absolute left-0 top-(--oy)" use:vars={{ '--oy': -scrollTop }}>
                {@render cells(panes.left)}
              </div>
            </div>
          {/if}
          {#if fw > 0 && fh > 0}
            <div class="absolute overflow-hidden left-(--l) top-(--t) w-(--w) h-(--h) border-r border-b border-base-content/30" use:vars={{ '--l': hdrW, '--t': hdrH, '--w': fw, '--h': fh }}>
              {@render cells(panes.corner)}
            </div>
          {/if}

          <!-- Column headers (A, B, C…) -->
          <div class="absolute overflow-hidden top-0 left-(--l) w-(--w) h-(--h) bg-base-200" use:vars={{ '--l': hdrW, '--w': Math.max(0, vw - hdrW), '--h': hdrH }}>
            {#each panes.colHeads as h (`${h.frozen}:${h.c}`)}
              {@const on = h.c >= sel.c1 && h.c <= sel.c2}
              <button
                tabindex="-1"
                class="absolute top-0 h-full left-(--x) w-(--w) flex items-center justify-center border-r border-b border-base-content/15 font-mono cursor-pointer {on ? 'bg-primary/15 text-primary font-semibold' : 'text-base-content/60 hover:bg-base-300'} {h.frozen ? 'z-10 bg-base-200' : ''}"
                use:vars={{ '--x': h.frozen ? h.x : h.x + fw - scrollLeft, '--w': h.w }}
                onclick={(e) => selectColumn(h.c, e.shiftKey)}
              >{colName(h.c)}</button>
            {/each}
          </div>
          <!-- Row headers (1, 2, 3…) -->
          <div class="absolute overflow-hidden left-0 top-(--t) w-(--w) h-(--h) bg-base-200" use:vars={{ '--t': hdrH, '--w': hdrW, '--h': Math.max(0, vh - hdrH) }}>
            {#each panes.rowHeads as h (`${h.frozen}:${h.r}`)}
              {@const on = h.r >= sel.r1 && h.r <= sel.r2}
              <button
                tabindex="-1"
                class="absolute left-0 w-full top-(--y) h-(--h) flex items-center justify-center border-r border-b border-base-content/15 font-mono cursor-pointer {on ? 'bg-primary/15 text-primary font-semibold' : 'text-base-content/60 hover:bg-base-300'} {h.frozen ? 'z-10 bg-base-200' : ''}"
                use:vars={{ '--y': h.frozen ? h.y : h.y + fh - scrollTop, '--h': h.h }}
                onclick={(e) => selectRow(h.r, e.shiftKey)}
              >{h.r}</button>
            {/each}
          </div>
          <div class="absolute left-0 top-0 w-(--w) h-(--h) bg-base-200 border-r border-b border-base-content/15 z-10" use:vars={{ '--w': hdrW, '--h': hdrH }}></div>
        </div>
      </div>
    </div>

    <!-- Status bar -->
    <div class="flex items-center gap-3 px-2 h-7 border-t border-base-300 bg-base-200 text-xs shrink-0">
      <div class="flex-1 min-w-0 truncate">
        {#if hint}
          <span class="text-base-content/70">{hint}</span>
        {:else if stale}
          <span class="text-error inline-flex items-center gap-2" role="alert">
            <span class="truncate">{editError?.message || saveError}</span>
            <button class="btn btn-xs btn-outline" onclick={reload}>{$t('common.refresh')}</button>
          </span>
        {:else if editError}
          <span class="text-error inline-flex items-center gap-1" role="alert">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
            {$t('sheet.editFailed', { values: { cell: `${editError.sheet}!${editError.cell}`, message: editError.message } })}
          </span>
        {:else if saveError}
          <span class="text-error" role="alert">{$t('sheet.saveFailed', { values: { message: saveError } })}</span>
        {:else if busy > 0}
          <span class="text-base-content/50">{$t('sheet.calculating')}</span>
        {:else if readOnly || !anyEditable}
          <span class="text-base-content/50">{$t('sheet.viewOnly')}</span>
        {:else if activeEditable}
          <span class="text-base-content/50">{$t('sheet.inputCell')}</span>
        {:else if dirty}
          <span class="text-base-content/50">{$t('sheet.unsaved')}</span>
        {/if}
      </div>
      {#if stats}
        <span class="text-base-content/70 font-mono whitespace-nowrap">{$t('sheet.sum')}: {numberFmt.format(stats.sum)}</span>
        <span class="text-base-content/70 font-mono whitespace-nowrap max-sm:hidden">{$t('sheet.average')}: {numberFmt.format(stats.average)}</span>
        <span class="text-base-content/70 font-mono whitespace-nowrap max-sm:hidden">{$t('sheet.count')}: {stats.count}</span>
      {/if}
      <div class="flex items-center shrink-0">
        <button class="btn btn-ghost btn-xs btn-square" onclick={() => zoomBy(-1)} title={$t('sheet.zoomOut')} aria-label={$t('sheet.zoomOut')}>−</button>
        <button class="btn btn-ghost btn-xs font-mono w-12" onclick={() => setZoom(1)} title={$t('sheet.zoomReset')}>{Math.round(zoom * 100)}%</button>
        <button class="btn btn-ghost btn-xs btn-square" onclick={() => zoomBy(1)} title={$t('sheet.zoomIn')} aria-label={$t('sheet.zoomIn')}>+</button>
      </div>
    </div>
  {/if}
</div>
