import { describe, expect, it } from 'vitest';
import fixture from './fixtures/sheet.json';
import type { SheetData, SheetViewModel } from './types';
import {
	Axis,
	DEFAULT_COL_PX,
	DEFAULT_ROW_PX,
	EditHistory,
	PAGE_ROWS,
	SheetModel,
	WorkbookModel,
	addr,
	cellLook,
	colIndex,
	colName,
	expandForMerges,
	inputOf,
	paneBoxes,
	parseAddr,
	parseRange,
	scrollToReveal,
	selectionStats,
	step,
	visibleWindow
} from './model';

const vm = fixture as unknown as SheetViewModel;
const book = () => new WorkbookModel(structuredClone(vm));

function bigSheet(rows: number, cols: number, extra: Partial<SheetData> = {}): SheetModel {
	return new SheetModel({ name: 'Big', dims: { rows, cols }, cells: [], ...extra });
}

describe('addresses', () => {
	it('round-trips column letters', () => {
		expect([1, 26, 27, 52, 703, 16384].map(colName)).toEqual(['A', 'Z', 'AA', 'AZ', 'AAA', 'XFD']);
		expect(['A', 'Z', 'AA', 'xfd'].map(colIndex)).toEqual([1, 26, 27, 16384]);
		expect(addr(9, 2)).toBe('B9');
	});

	it('parses absolute, sheet-qualified and quoted refs', () => {
		expect(parseAddr('$B$9')).toEqual({ sheet: undefined, r: 9, c: 2 });
		expect(parseAddr('Inputs!$B$9')).toEqual({ sheet: 'Inputs', r: 9, c: 2 });
		expect(parseAddr("'Monthly Model'!C20")).toEqual({ sheet: 'Monthly Model', r: 20, c: 3 });
		expect(parseAddr('nope')).toBeNull();
		expect(parseRange('E3:A1')).toEqual({ r1: 1, c1: 1, r2: 3, c2: 5 });
	});
});

describe('geometry', () => {
	it('applies pixel widths, heights and zoom, defaulting to Excel sizes', () => {
		const inputs = book().sheet('Inputs')!;
		expect(inputs.cols.size(1)).toBe(224);
		expect(inputs.cols.size(3)).toBe(DEFAULT_COL_PX);
		expect(inputs.rows.size(1)).toBe(32);
		expect(inputs.rows.size(2)).toBe(DEFAULT_ROW_PX);
		inputs.layout(2);
		expect(inputs.rows.size(2)).toBe(DEFAULT_ROW_PX * 2);
		expect(inputs.cols.size(3)).toBe(DEFAULT_COL_PX * 2);
	});

	it('finds the index under a pixel by binary search', () => {
		const a = new Axis(5, (i) => i * 10); // 10,20,30,40,50 → starts 0,10,30,60,100
		expect([0, 9, 10, 29, 30, 99, 100, 149, 9999].map((px) => a.indexAt(px))).toEqual([1, 1, 2, 2, 3, 4, 5, 5, 5]);
		expect(a.total).toBe(150);
		expect(a.start(4)).toBe(60);
	});
});

describe('virtualization', () => {
	it('shows only the rows and columns in view, plus overscan', () => {
		const s = bigSheet(100_000, 200);
		const w = visibleWindow(s, { scrollTop: 0, scrollLeft: 0, width: 640, height: 400 }, 2);
		expect(w).toEqual({ r1: 1, r2: 23, c1: 1, c2: 13 });
		const far = visibleWindow(s, { scrollTop: DEFAULT_ROW_PX * 5000, scrollLeft: DEFAULT_COL_PX * 100, width: 640, height: 400 }, 0);
		expect(far.r1).toBe(5001);
		expect(far.r2).toBe(5021);
		expect(far.c1).toBe(101);
		expect(far.c2).toBe(111);
	});

	it('never puts frozen rows/columns in the scrolling window', () => {
		const s = bigSheet(1000, 50, { freeze: { rows: 2, cols: 1 } });
		const w = visibleWindow(s, { scrollTop: 0, scrollLeft: 0, width: 300, height: 100 }, 3);
		expect(w.r1).toBe(3);
		expect(w.c1).toBe(2);
		expect(s.frozenHeight).toBe(DEFAULT_ROW_PX * 2);
		expect(s.frozenWidth).toBe(DEFAULT_COL_PX);
	});

	it('clamps at the end of the sheet', () => {
		const s = bigSheet(10, 3);
		const w = visibleWindow(s, { scrollTop: 0, scrollLeft: 0, width: 5000, height: 5000 });
		expect(w).toEqual({ r1: 1, r2: 10, c1: 1, c2: 3 });
	});

	it('positions pane boxes relative to the pane origin', () => {
		const s = book().sheet('Monthly Model')!;
		const boxes = paneBoxes(s, [2, 3], [2, 3], 2, 2);
		expect(boxes.map((b) => [addr(b.r, b.c), b.x, b.y])).toEqual([
			['B2', 0, 0],
			['C2', DEFAULT_COL_PX, 0],
			['B3', 0, DEFAULT_ROW_PX],
			['C3', DEFAULT_COL_PX, DEFAULT_ROW_PX]
		]);
		expect(boxes[0].cell?.display).toBe('$3,002');
	});

	it('scrolls just enough to reveal a cell, never for frozen ones', () => {
		const s = bigSheet(1000, 50, { freeze: { rows: 1, cols: 1 } });
		const vp = { scrollTop: 0, scrollLeft: 0, width: 300, height: 100 };
		expect(scrollToReveal(s, vp, 1, 1)).toEqual({ scrollTop: 0, scrollLeft: 0 });
		const r = scrollToReveal(s, vp, 50, 2);
		expect(r.scrollTop).toBe(DEFAULT_ROW_PX * 49 - 100);
		expect(scrollToReveal(s, { ...vp, scrollTop: 500 }, 3, 2).scrollTop).toBe(DEFAULT_ROW_PX);
	});
});

describe('merges', () => {
	it('draws a merge once at full size and skips the cells it covers', () => {
		const s = book().sheet('Inputs')!;
		const boxes = paneBoxes(s, [1, 1], [1, 5], 1, 1);
		expect(boxes).toHaveLength(1);
		expect(boxes[0].w).toBe(224 + 84 + DEFAULT_COL_PX * 3);
		expect(boxes[0].cell?.display).toBe('Growth assumptions');
	});

	it('still draws a merge whose origin scrolled out of the window', () => {
		const s = book().sheet('Inputs')!;
		const boxes = paneBoxes(s, [1, 1], [3, 5], 1, 2);
		expect(boxes).toHaveLength(1);
		expect(addr(boxes[0].r, boxes[0].c)).toBe('A1');
		// At its true origin, left of this pane (which clips it).
		expect(boxes[0].x).toBe(-224);
	});

	it('navigates over a merge and anchors clicks to its origin', () => {
		const s = book().sheet('Inputs')!;
		expect(s.anchorOf(1, 4)).toEqual({ r: 1, c: 1 });
		expect(step(s, { r: 1, c: 1 }, 0, 1)).toEqual({ r: 1, c: 1 }); // E is the last column
		expect(step(s, { r: 1, c: 1 }, 1, 0)).toEqual({ r: 2, c: 1 });
		expect(step(s, { r: 2, c: 3 }, -1, 0)).toEqual({ r: 1, c: 1 });
		expect(expandForMerges(s, { r1: 1, c1: 2, r2: 2, c2: 2 })).toEqual({ r1: 1, c1: 1, r2: 2, c2: 5 });
	});
});

describe('editing', () => {
	it('lets only editable cells take input', () => {
		const s = book().sheet('Inputs')!;
		expect(s.isEditable(9, 2)).toBe(true); // ARPU
		expect(s.isEditable(9, 1)).toBe(false); // label
		expect(s.isEditable(10, 2)).toBe(false); // formula
		expect(s.isEditable(20, 20)).toBe(false); // empty
		expect(s.isEditable(1, 3)).toBe(false); // inside a locked merge
	});

	it('starts an edit from the formula, else the raw value', () => {
		const s = book().sheet('Inputs')!;
		expect(inputOf(s.get(4, 2))).toBe('0.05');
		expect(inputOf(s.get(10, 2))).toBe('=B9/0');
		expect(inputOf(s.get(11, 2))).toBe('TRUE');
		expect(inputOf(undefined)).toBe('');
	});

	it('applies changed cells live across sheets', () => {
		const b = book();
		const touched = b.applyChanged([
			{ sheet: 'Inputs', cell: 'B9', display: '$40', value: 40 },
			{ sheet: 'Monthly Model', cell: 'B2', display: '$4,002', value: 4002 },
			{ sheet: 'Monthly Model', cell: 'Z900', display: '7', value: 7 },
			{ sheet: 'Gone', cell: 'A1', display: 'x', value: 'x' }
		]);
		expect([...touched].sort()).toEqual(['Inputs', 'Monthly Model']);
		const inputs = b.sheet('Inputs')!;
		expect(inputs.get(9, 2)).toMatchObject({ display: '$40', value: 40, editable: true, style: 4 });
		const model = b.sheet('Monthly Model')!;
		expect(model.get(2, 2)).toMatchObject({ display: '$4,002', formula: '=Inputs!$B$3*Inputs!$B$9+2' });
		expect(model.get(900, 26)).toMatchObject({ display: '7', editable: false });
	});

	it('undoes and redoes the session by re-sending inputs', () => {
		const h = new EditHistory();
		h.record({ sheet: 'Inputs', cell: 'B9', before: '30', after: '40' });
		h.record({ sheet: 'Inputs', cell: 'B4', before: '0.05', after: '0.07' });
		h.record({ sheet: 'Inputs', cell: 'B3', before: '100', after: '100' }); // no-op, not recorded
		expect(h.undo()).toEqual({ sheet: 'Inputs', cell: 'B4', input: '0.05' });
		expect(h.undo()).toEqual({ sheet: 'Inputs', cell: 'B9', input: '30' });
		expect(h.undo()).toBeUndefined();
		expect(h.redo()).toEqual({ sheet: 'Inputs', cell: 'B9', input: '40' });
		h.record({ sheet: 'Inputs', cell: 'B3', before: '100', after: '120' });
		expect(h.canRedo).toBe(false);
		expect(h.undo()).toEqual({ sheet: 'Inputs', cell: 'B3', input: '100' });
		h.revertUndo();
		expect(h.past.at(-1)?.cell).toBe('B3');
	});
});

describe('workbook', () => {
	it('omits hidden sheets from the tabs', () => {
		expect(book().visibleSheets.map((s) => s.name)).toEqual(['Inputs', 'Monthly Model']);
	});

	it('names a cell by its defined name', () => {
		const b = book();
		expect(b.nameFor('Inputs', 9, 2)).toBe('ARPU');
		expect(b.nameFor('Monthly Model', 9, 2)).toBeUndefined();
	});

	it('finds display text and formulas across sheets in order', () => {
		const b = book();
		expect(b.find('arpu')[0]).toEqual({ sheet: 'Inputs', r: 9, c: 1 });
		const formulaHits = b.find('inputs!$b$4');
		expect(formulaHits.length).toBeGreaterThan(0);
		expect(formulaHits.every((h) => h.sheet === 'Monthly Model')).toBe(true);
		expect(b.find('lookup-only')).toEqual([]);
		expect(b.find('x')).not.toContainEqual({ sheet: 'Lookup', r: 1, c: 1 });
	});

	it('sums, averages and counts only numbers', () => {
		const s = book().sheet('Inputs')!;
		expect(selectionStats(s, { r1: 3, c1: 1, r2: 11, c2: 2 })).toEqual({ sum: 130.05, count: 3, average: 130.05 / 3 });
	});
});

describe('paging', () => {
	it('asks only for pages not yet loaded', () => {
		const s = bigSheet(PAGE_ROWS * 4, 5);
		s.markPage(0, [{ r: 1, c: 1, display: 'a', value: 'a', formula: null, editable: false }]);
		expect(s.fullyLoaded).toBe(false);
		expect(s.missingPages(400, 1200)).toEqual([1, 2]);
		s.markPage(1, []);
		s.markPage(2, []);
		s.markPage(3, []);
		expect(s.fullyLoaded).toBe(true);
		expect(s.missingPages(1, PAGE_ROWS * 4)).toEqual([]);
	});

	it('treats a response with rows outside the page as the whole sheet', () => {
		const s = bigSheet(PAGE_ROWS * 4, 5);
		s.markPage(0, [{ r: PAGE_ROWS * 3, c: 1, display: 'z', value: 'z', formula: null, editable: false }]);
		expect(s.fullyLoaded).toBe(true);
	});

	it('a small sheet is complete after its first page', () => {
		const s = bigSheet(30, 5);
		s.markPage(0, []);
		expect(s.fullyLoaded).toBe(true);
	});
});

describe('cell look', () => {
	it('right-aligns numbers, centres booleans and errors, honours the style', () => {
		const s = book().sheet('Inputs')!;
		expect(cellLook(undefined, s.get(9, 2)).align).toBe('right');
		expect(cellLook(undefined, s.get(11, 2)).align).toBe('center');
		expect(cellLook(undefined, s.get(10, 2)).align).toBe('center');
		expect(cellLook(undefined, s.get(9, 1)).align).toBe('left');
		expect(cellLook({ align: 'center' }, s.get(9, 2)).align).toBe('center');
		expect(cellLook({ valign: 'top', size: 14 }, s.get(9, 2))).toMatchObject({ valign: 'top', size: 14 });
		expect(cellLook(undefined, s.get(9, 2))).toMatchObject({ valign: 'bottom', size: undefined });
	});

	it('keeps the file colours readable in any theme', () => {
		// Automatic black ink with no fill follows the theme's text colour.
		expect(cellLook({ color: '#000000' }, undefined).ink).toBeUndefined();
		// A real colour stays where it reads, and follows the theme where it wouldn't.
		expect(cellLook({ color: '#0000FF' }, undefined).ink).toBe('#0000FF');
		expect(cellLook({ color: '#0000FF' }, undefined, true).ink).toBeUndefined();
		expect(cellLook({ color: '#FFC000' }, undefined, true).ink).toBe('#FFC000');
		// Fill keeps its readable ink, and gets one when the file's would vanish.
		expect(cellLook({ fill: '#EDD8A0', color: '#1F1F1F' }, undefined)).toMatchObject({ fill: '#EDD8A0', ink: '#1F1F1F' });
		expect(cellLook({ fill: '#1F3864' }, undefined).ink).toBe('#ffffff');
		expect(cellLook({ fill: '#FFFF00', color: '#FFFFFF' }, undefined).ink).toBe('#000000');
	});
});
