/**
 * The grid model: pure, DOM-free logic behind SheetView — addresses,
 * geometry (widths/heights/zoom as prefix sums), virtualization windows,
 * freeze panes, merges, editable gating, applying engine results, the
 * session's undo stack, selection stats and find. Everything here is unit
 * tested against fixtures/sheet.json; the component only wires it to the DOM.
 */
import type { CellStyle, ChangedCell, SheetCell, SheetData, SheetViewModel } from './types';

// ── Addresses ────────────────────────────────────────────────────────────

/** 1 → "A", 27 → "AA". */
export function colName(c: number): string {
	let s = '';
	for (let n = c; n > 0; n = Math.floor((n - 1) / 26)) s = String.fromCharCode(65 + ((n - 1) % 26)) + s;
	return s;
}

/** "A" → 1, "AA" → 27. */
export function colIndex(name: string): number {
	let n = 0;
	for (const ch of name.toUpperCase()) n = n * 26 + (ch.charCodeAt(0) - 64);
	return n;
}

export function addr(r: number, c: number): string {
	return `${colName(c)}${r}`;
}

/** "B9", "$B$9" or "Inputs!$B$9" → {sheet?, r, c}. */
export function parseAddr(ref: string): { sheet?: string; r: number; c: number } | null {
	const bang = ref.lastIndexOf('!');
	const sheet = bang >= 0 ? ref.slice(0, bang).replace(/^'|'$/g, '').replace(/''/g, "'") : undefined;
	const m = ref.slice(bang + 1).match(/^\$?([A-Za-z]{1,3})\$?(\d+)$/);
	if (!m) return null;
	return { sheet, r: Number(m[2]), c: colIndex(m[1]) };
}

export interface Rect {
	r1: number;
	c1: number;
	r2: number;
	c2: number;
}

/** "A1:E3" (or a single "B2") → a normalized rect. */
export function parseRange(ref: string): Rect | null {
	const [a, b = a] = ref.split(':');
	const p = parseAddr(a);
	const q = parseAddr(b);
	if (!p || !q) return null;
	return {
		r1: Math.min(p.r, q.r),
		c1: Math.min(p.c, q.c),
		r2: Math.max(p.r, q.r),
		c2: Math.max(p.c, q.c)
	};
}

export function rectOf(a: { r: number; c: number }, b: { r: number; c: number }): Rect {
	return { r1: Math.min(a.r, b.r), c1: Math.min(a.c, b.c), r2: Math.max(a.r, b.r), c2: Math.max(a.c, b.c) };
}

export function rangeName(rect: Rect): string {
	const a = addr(rect.r1, rect.c1);
	return rect.r1 === rect.r2 && rect.c1 === rect.c2 ? a : `${a}:${addr(rect.r2, rect.c2)}`;
}

// ── Units ────────────────────────────────────────────────────────────────

/** Excel's default column (8.43 characters) and row (15pt) in CSS px. The
 *  engine sends every other size in px already. */
export const DEFAULT_COL_PX = 64;
export const DEFAULT_ROW_PX = 20;

// ── Axis: prefix sums over 1-based sizes ─────────────────────────────────

export class Axis {
	readonly count: number;
	/** offsets[i] = start of index i+1; offsets[count] = total. */
	private offsets: Float64Array;

	constructor(count: number, sizeOf: (i: number) => number) {
		this.count = count;
		this.offsets = new Float64Array(count + 1);
		for (let i = 1; i <= count; i++) this.offsets[i] = this.offsets[i - 1] + sizeOf(i);
	}

	/** Start of 1-based index i (i = count+1 gives the total). */
	start(i: number): number {
		return this.offsets[Math.max(0, Math.min(this.count, i - 1))];
	}

	size(i: number): number {
		return this.start(i + 1) - this.start(i);
	}

	get total(): number {
		return this.offsets[this.count];
	}

	/** The 1-based index whose span holds px (clamped to 1..count). */
	indexAt(px: number): number {
		if (this.count === 0) return 0;
		let lo = 0;
		let hi = this.count - 1;
		while (lo < hi) {
			const mid = (lo + hi + 1) >> 1;
			if (this.offsets[mid] <= px) lo = mid;
			else hi = mid - 1;
		}
		return lo + 1;
	}
}

// ── One sheet ────────────────────────────────────────────────────────────

/** Cells are keyed as one number: 16384 columns is Excel's limit. */
export function cellKey(r: number, c: number): number {
	return r * 16384 + c;
}

export const PAGE_ROWS = 500;

export class SheetModel {
	readonly data: SheetData;
	readonly name: string;
	readonly cells = new Map<number, SheetCell>();
	readonly merges: Rect[] = [];
	/** Every merged cell → its merge (origin included). */
	private mergeIndex = new Map<number, Rect>();
	readonly freezeRows: number;
	readonly freezeCols: number;
	rows!: Axis;
	cols!: Axis;
	zoom = 1;
	/** Row pages (PAGE_ROWS each, 0-based page number) already fetched. */
	readonly loadedPages = new Set<number>();
	/** True once every row's cells are here. */
	fullyLoaded = false;

	constructor(data: SheetData) {
		this.data = data;
		this.name = data.name;
		for (const cell of data.cells) this.cells.set(cellKey(cell.r, cell.c), cell);
		for (const m of data.merges ?? []) {
			const rect = parseRange(m);
			if (!rect) continue;
			this.merges.push(rect);
			for (let r = rect.r1; r <= rect.r2; r++)
				for (let c = rect.c1; c <= rect.c2; c++) this.mergeIndex.set(cellKey(r, c), rect);
		}
		this.freezeRows = Math.max(0, data.freeze?.rows ?? 0);
		this.freezeCols = Math.max(0, data.freeze?.cols ?? 0);
		this.layout(1);
	}

	get rowCount(): number {
		let max = this.data.dims?.rows ?? 0;
		for (const cell of this.cells.values()) if (cell.r > max) max = cell.r;
		for (const m of this.merges) if (m.r2 > max) max = m.r2;
		return Math.max(1, max, this.freezeRows);
	}

	get colCount(): number {
		let max = this.data.dims?.cols ?? 0;
		for (const cell of this.cells.values()) if (cell.c > max) max = cell.c;
		for (const m of this.merges) if (m.c2 > max) max = m.c2;
		return Math.max(1, max, this.freezeCols);
	}

	/** (Re)build geometry at a zoom factor. */
	layout(zoom: number): void {
		this.zoom = zoom;
		const widths = this.data.colWidths ?? {};
		const heights = this.data.rowHeights ?? {};
		const colByIndex = new Map<number, number>();
		for (const [k, v] of Object.entries(widths)) colByIndex.set(colIndex(k), v);
		this.cols = new Axis(this.colCount, (c) =>
			Math.round((colByIndex.get(c) ?? DEFAULT_COL_PX) * zoom)
		);
		this.rows = new Axis(this.rowCount, (r) =>
			Math.round((heights[String(r)] ?? DEFAULT_ROW_PX) * zoom)
		);
	}

	get(r: number, c: number): SheetCell | undefined {
		return this.cells.get(cellKey(r, c));
	}

	mergeAt(r: number, c: number): Rect | undefined {
		return this.mergeIndex.get(cellKey(r, c));
	}

	/** The cell a click on (r,c) lands on: a merge's top-left. */
	anchorOf(r: number, c: number): { r: number; c: number } {
		const m = this.mergeAt(r, c);
		return m ? { r: m.r1, c: m.c1 } : { r, c };
	}

	/** Only cells the engine marked editable accept input; everything else is view-only. */
	isEditable(r: number, c: number): boolean {
		const a = this.anchorOf(r, c);
		return this.get(a.r, a.c)?.editable === true;
	}

	/** Pixel size of the frozen block (rows above / columns left of the split). */
	get frozenHeight(): number {
		return this.rows.start(this.freezeRows + 1);
	}

	get frozenWidth(): number {
		return this.cols.start(this.freezeCols + 1);
	}

	/** Merge loaded cells in (a page, a reload). */
	upsert(cells: SheetCell[]): void {
		for (const cell of cells) this.cells.set(cellKey(cell.r, cell.c), cell);
	}

	/** Pages still needed to show rows r1..r2. */
	missingPages(r1: number, r2: number): number[] {
		if (this.fullyLoaded) return [];
		const out: number[] = [];
		for (let p = Math.floor((r1 - 1) / PAGE_ROWS); p <= Math.floor((r2 - 1) / PAGE_ROWS); p++) {
			if (!this.loadedPages.has(p)) out.push(p);
		}
		return out;
	}

	/** Record a fetched page. A response carrying rows outside it means the
	 *  server sent everything (it ignores paging), so the sheet is complete. */
	markPage(page: number, cells: SheetCell[]): void {
		this.loadedPages.add(page);
		const lo = page * PAGE_ROWS + 1;
		const hi = (page + 1) * PAGE_ROWS;
		if (cells.some((c) => c.r < lo || c.r > hi)) this.fullyLoaded = true;
		if (this.data.dims.rows <= hi && page === 0) this.fullyLoaded = true;
		const pages = Math.ceil(this.data.dims.rows / PAGE_ROWS);
		if (this.loadedPages.size >= pages) this.fullyLoaded = true;
	}
}

// ── Virtualization ───────────────────────────────────────────────────────

export interface Viewport {
	/** Scroll offsets of the scrolling (unfrozen) region. */
	scrollTop: number;
	scrollLeft: number;
	/** Size of the scrolling region on screen (excluding headers and frozen panes). */
	width: number;
	height: number;
}

export interface Window {
	r1: number;
	r2: number;
	c1: number;
	c2: number;
}

/** The unfrozen rows/columns a viewport shows, plus `overscan` on each side. */
export function visibleWindow(sheet: SheetModel, vp: Viewport, overscan = 2): Window {
	const fr = sheet.freezeRows;
	const fc = sheet.freezeCols;
	const top = sheet.frozenHeight + vp.scrollTop;
	const left = sheet.frozenWidth + vp.scrollLeft;
	const r1 = Math.max(fr + 1, sheet.rows.indexAt(top) - overscan);
	const r2 = Math.min(sheet.rows.count, sheet.rows.indexAt(top + Math.max(0, vp.height)) + overscan);
	const c1 = Math.max(fc + 1, sheet.cols.indexAt(left) - overscan);
	const c2 = Math.min(sheet.cols.count, sheet.cols.indexAt(left + Math.max(0, vp.width)) + overscan);
	return { r1, r2: Math.max(r1 - 1, r2), c1, c2: Math.max(c1 - 1, c2) };
}

/** One cell box to draw, positioned relative to its pane's origin. */
export interface CellBox {
	key: number;
	r: number;
	c: number;
	x: number;
	y: number;
	w: number;
	h: number;
	cell?: SheetCell;
	merge?: Rect;
}

/**
 * The boxes of one pane: rows r1..r2 × cols c1..c2, offset so the pane's
 * first row/col (originR/originC) sits at 0. Cells covered by a merge are
 * skipped; a merge whose top-left falls outside the window (or the pane) but
 * which reaches into it is still drawn (once, at full size, at its origin), so
 * merged headers never vanish while scrolling.
 */
export function paneBoxes(
	sheet: SheetModel,
	rows: [number, number],
	cols: [number, number],
	originR: number,
	originC: number
): CellBox[] {
	const out: CellBox[] = [];
	const [r1, r2] = rows;
	const [c1, c2] = cols;
	if (r2 < r1 || c2 < c1) return out;
	const oy = sheet.rows.start(originR);
	const ox = sheet.cols.start(originC);
	const drawn = new Set<Rect>();
	for (let r = r1; r <= r2; r++) {
		for (let c = c1; c <= c2; c++) {
			const m = sheet.mergeAt(r, c);
			if (m) {
				if (drawn.has(m)) continue;
				drawn.add(m);
				// Draw the whole merge at its true origin; the pane clips it. A
				// merge across the freeze line shows its text once (in the pane
				// holding its origin) and its fill in both.
				out.push({
					key: cellKey(m.r1, m.c1),
					r: m.r1,
					c: m.c1,
					x: sheet.cols.start(m.c1) - ox,
					y: sheet.rows.start(m.r1) - oy,
					w: sheet.cols.start(m.c2 + 1) - sheet.cols.start(m.c1),
					h: sheet.rows.start(m.r2 + 1) - sheet.rows.start(m.r1),
					cell: sheet.get(m.r1, m.c1),
					merge: m
				});
				continue;
			}
			out.push({
				key: cellKey(r, c),
				r,
				c,
				x: sheet.cols.start(c) - ox,
				y: sheet.rows.start(r) - oy,
				w: sheet.cols.size(c),
				h: sheet.rows.size(r),
				cell: sheet.get(r, c)
			});
		}
	}
	return out;
}

/** Scroll offsets that bring (r,c) fully into the scrolling region (frozen cells never scroll). */
export function scrollToReveal(sheet: SheetModel, vp: Viewport, r: number, c: number): { scrollTop: number; scrollLeft: number } {
	let { scrollTop, scrollLeft } = vp;
	if (r > sheet.freezeRows) {
		const y = sheet.rows.start(r) - sheet.frozenHeight;
		const h = sheet.rows.size(r);
		if (y < scrollTop) scrollTop = y;
		else if (y + h > scrollTop + vp.height) scrollTop = Math.max(0, y + h - vp.height);
	}
	if (c > sheet.freezeCols) {
		const x = sheet.cols.start(c) - sheet.frozenWidth;
		const w = sheet.cols.size(c);
		if (x < scrollLeft) scrollLeft = x;
		else if (x + w > scrollLeft + vp.width) scrollLeft = Math.max(0, x + w - vp.width);
	}
	return { scrollTop, scrollLeft };
}

// ── Navigation ───────────────────────────────────────────────────────────

/** One step from (r,c) by (dr,dc), stepping over a whole merge and landing on a merge's top-left. */
export function step(sheet: SheetModel, at: { r: number; c: number }, dr: number, dc: number): { r: number; c: number } {
	const m = sheet.mergeAt(at.r, at.c);
	let r = at.r;
	let c = at.c;
	if (dr > 0) r = (m ? m.r2 : r) + dr;
	else if (dr < 0) r = (m ? m.r1 : r) + dr;
	if (dc > 0) c = (m ? m.c2 : c) + dc;
	else if (dc < 0) c = (m ? m.c1 : c) + dc;
	r = Math.max(1, Math.min(sheet.rows.count, r));
	c = Math.max(1, Math.min(sheet.cols.count, c));
	return sheet.anchorOf(r, c);
}

/** A selection rect grown to swallow every merge it touches. */
export function expandForMerges(sheet: SheetModel, rect: Rect): Rect {
	const out = { ...rect };
	let grew = true;
	while (grew) {
		grew = false;
		for (const m of sheet.merges) {
			const hits = m.r1 <= out.r2 && m.r2 >= out.r1 && m.c1 <= out.c2 && m.c2 >= out.c1;
			if (!hits) continue;
			if (m.r1 < out.r1 || m.r2 > out.r2 || m.c1 < out.c1 || m.c2 > out.c2) {
				out.r1 = Math.min(out.r1, m.r1);
				out.r2 = Math.max(out.r2, m.r2);
				out.c1 = Math.min(out.c1, m.c1);
				out.c2 = Math.max(out.c2, m.c2);
				grew = true;
			}
		}
	}
	return out;
}

// ── Selection stats ──────────────────────────────────────────────────────

export function selectionStats(sheet: SheetModel, rect: Rect): { sum: number; count: number; average: number } {
	let sum = 0;
	let count = 0;
	const area = (rect.r2 - rect.r1 + 1) * (rect.c2 - rect.c1 + 1);
	const take = (cell: SheetCell | undefined) => {
		if (cell && typeof cell.value === 'number' && Number.isFinite(cell.value)) {
			sum += cell.value;
			count++;
		}
	};
	if (area <= sheet.cells.size) {
		for (let r = rect.r1; r <= rect.r2; r++) for (let c = rect.c1; c <= rect.c2; c++) take(sheet.get(r, c));
	} else {
		for (const cell of sheet.cells.values()) {
			if (cell.r >= rect.r1 && cell.r <= rect.r2 && cell.c >= rect.c1 && cell.c <= rect.c2) take(cell);
		}
	}
	return { sum, count, average: count ? sum / count : 0 };
}

// ── Editing ──────────────────────────────────────────────────────────────

/** What an edit box starts with: the formula, else the raw value. */
export function inputOf(cell: SheetCell | undefined): string {
	if (!cell) return '';
	if (cell.formula) return cell.formula;
	if (cell.value == null) return '';
	if (typeof cell.value === 'boolean') return cell.value ? 'TRUE' : 'FALSE';
	return String(cell.value);
}

export interface EditRecord {
	sheet: string;
	cell: string;
	before: string;
	after: string;
}

/** The session's edits, for undo/redo. Undo re-sends `before`; redo re-sends `after`. */
export class EditHistory {
	past: EditRecord[] = [];
	future: EditRecord[] = [];

	record(e: EditRecord): void {
		if (e.before === e.after) return;
		this.past.push(e);
		this.future = [];
	}

	get canUndo(): boolean {
		return this.past.length > 0;
	}

	get canRedo(): boolean {
		return this.future.length > 0;
	}

	/** The edit to send to undo the last change (moves it to the redo side). */
	undo(): { sheet: string; cell: string; input: string } | undefined {
		const e = this.past.pop();
		if (!e) return undefined;
		this.future.push(e);
		return { sheet: e.sheet, cell: e.cell, input: e.before };
	}

	redo(): { sheet: string; cell: string; input: string } | undefined {
		const e = this.future.pop();
		if (!e) return undefined;
		this.past.push(e);
		return { sheet: e.sheet, cell: e.cell, input: e.after };
	}

	/** A re-send failed: put the record back where it came from. */
	revertUndo(): void {
		const e = this.future.pop();
		if (e) this.past.push(e);
	}

	revertRedo(): void {
		const e = this.past.pop();
		if (e) this.future.push(e);
	}
}

// ── The workbook ─────────────────────────────────────────────────────────

export class WorkbookModel {
	readonly documentId: string;
	readonly version: number;
	readonly styles: CellStyle[];
	readonly names: Record<string, string>;
	readonly sheets: SheetModel[];

	constructor(vm: SheetViewModel) {
		this.documentId = vm.documentId;
		this.version = vm.version;
		this.styles = vm.styles ?? [];
		this.names = vm.names ?? {};
		this.sheets = vm.sheets.map((s) => new SheetModel(s));
	}

	/** Tabs: hidden sheets are omitted. */
	get visibleSheets(): SheetModel[] {
		return this.sheets.filter((s) => !s.data.hidden);
	}

	sheet(name: string): SheetModel | undefined {
		return this.sheets.find((s) => s.name === name);
	}

	/**
	 * Apply the engine's recalculated cells (any sheet). A changed cell that
	 * isn't loaded yet (a page not fetched) is created view-only; its page
	 * fetch later brings the full record. Returns the sheets touched.
	 */
	applyChanged(changed: ChangedCell[]): Set<string> {
		const touched = new Set<string>();
		for (const ch of changed) {
			const sheet = this.sheet(ch.sheet);
			const at = parseAddr(ch.cell);
			if (!sheet || !at) continue;
			const prev = sheet.get(at.r, at.c);
			const next: SheetCell = prev
				? { ...prev, display: ch.display, value: ch.value }
				: { r: at.r, c: at.c, display: ch.display, value: ch.value, formula: null, style: null, editable: false };
			if (ch.formula !== undefined) next.formula = ch.formula;
			sheet.cells.set(cellKey(at.r, at.c), next);
			touched.add(sheet.name);
		}
		return touched;
	}

	/**
	 * A single-cell defined name for (sheet, r, c), for the formula bar's name
	 * box. Sheet-scoped names are keyed "Sheet!Name" and only count on that
	 * sheet; a workbook name loses to a sheet-scoped one.
	 */
	nameFor(sheet: string, r: number, c: number): string | undefined {
		let found: string | undefined;
		for (const [key, ref] of Object.entries(this.names)) {
			const bang = key.lastIndexOf('!');
			const scope = bang >= 0 ? key.slice(0, bang).replace(/^'|'$/g, '') : undefined;
			if (scope !== undefined && scope !== sheet) continue;
			const p = parseAddr(ref);
			if (!p || p.r !== r || p.c !== c || (p.sheet ?? sheet) !== sheet) continue;
			if (scope !== undefined) return key.slice(bang + 1);
			found ??= key;
		}
		return found;
	}

	/** Where a defined name points, as typed in the name box on `sheet`. */
	resolveName(sheet: string, name: string): string | undefined {
		return this.names[`${sheet}!${name}`] ?? this.names[`'${sheet}'!${name}`] ?? this.names[name];
	}

	/** Find across every visible sheet's loaded cells: display text and formulas, case-insensitive, in tab then row then column order. */
	find(query: string): { sheet: string; r: number; c: number }[] {
		const q = query.trim().toLowerCase();
		if (!q) return [];
		const out: { sheet: string; r: number; c: number }[] = [];
		for (const s of this.visibleSheets) {
			const hits: SheetCell[] = [];
			for (const cell of s.cells.values()) {
				if (cell.display.toLowerCase().includes(q) || (cell.formula ?? '').toLowerCase().includes(q)) hits.push(cell);
			}
			hits.sort((a, b) => a.r - b.r || a.c - b.c);
			for (const h of hits) out.push({ sheet: s.name, r: h.r, c: h.c });
		}
		return out;
	}
}

// ── Cell look ────────────────────────────────────────────────────────────

function luminance(hex: string): number | null {
	const m = hex.match(/^#?([0-9a-f]{6})$/i);
	if (!m) return null;
	const n = parseInt(m[1], 16);
	const ch = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => {
		const s = v / 255;
		return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
	});
	return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2];
}

export type Align = 'left' | 'center' | 'right';
export type VAlign = 'top' | 'center' | 'bottom';

const ERRORS = /^#(NULL!|DIV\/0!|VALUE!|REF!|NAME\?|NUM!|N\/A|SPILL!|CALC!|CIRC!|ERROR!|N\/IMPL!)$/;

/** An error cell: the engine sends the error's text as its value. */
export function isError(cell: SheetCell | undefined): boolean {
	return !!cell && typeof cell.value === 'string' && ERRORS.test(cell.value);
}

export interface CellLook {
	align: Align;
	valign: VAlign;
	/** The file's font size in points, when it isn't the default. */
	size?: number;
	bold: boolean;
	italic: boolean;
	underline: boolean;
	wrap: boolean;
	/** The file's fill, or undefined for the theme's surface. */
	fill?: string;
	/** The file's ink, or undefined for the theme's text colour. */
	ink?: string;
}

/**
 * How a cell looks, from its style and value. Data, not design: the colours
 * are the file's own. Two adjustments keep every theme readable: ink on an
 * unfilled cell that wouldn't read against the theme's surface (black on a
 * dark theme, white on a light one, Excel's input blue on dark) follows the
 * theme's text colour, and ink on a fill that would be unreadable is replaced
 * by black or white by the fill's luminance.
 */
export function cellLook(style: CellStyle | undefined, cell: SheetCell | undefined, dark = false): CellLook {
	const s = style ?? {};
	let align: Align = 'left';
	if (s.align === 'left' || s.align === 'center' || s.align === 'right') align = s.align;
	else if (typeof cell?.value === 'number') align = 'right';
	else if (typeof cell?.value === 'boolean' || isError(cell)) align = 'center';
	const valign: VAlign = s.valign === 'top' || s.valign === 'center' ? s.valign : 'bottom';
	const look: CellLook = {
		align,
		valign,
		size: s.size && s.size !== 11 ? s.size : undefined,
		bold: !!s.bold,
		italic: !!s.italic,
		underline: !!s.underline,
		wrap: !!s.wrap
	};
	const fillLum = s.fill ? luminance(s.fill) : null;
	const inkLum = s.color ? luminance(s.color) : null;
	if (fillLum != null) {
		look.fill = s.fill!.startsWith('#') ? s.fill! : `#${s.fill}`;
		const ink = inkLum ?? 0;
		const contrast = (Math.max(fillLum, ink) + 0.05) / (Math.min(fillLum, ink) + 0.05);
		if (inkLum != null && contrast >= 3) look.ink = s.color!.startsWith('#') ? s.color! : `#${s.color}`;
		else look.ink = fillLum > 0.4 ? '#000000' : '#ffffff';
	} else if (inkLum != null) {
		// Surface luminance: the light themes' white, the dark themes' near-black.
		const surface = dark ? 0.015 : 1;
		const contrast = (Math.max(surface, inkLum) + 0.05) / (Math.min(surface, inkLum) + 0.05);
		const automatic = inkLum <= 0.02 || inkLum >= 0.85; // Excel's black/white "automatic" ink
		if (!automatic && contrast >= 3) look.ink = s.color!.startsWith('#') ? s.color! : `#${s.color}`;
	}
	return look;
}
