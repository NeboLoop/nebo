/**
 * Read-only fallback: build the same view model in the browser with SheetJS
 * for a spreadsheet the sheet engine can't serve — a file opened by its path
 * (`path:<url>`, no work document behind it), or a backend without the sheet
 * endpoint. Values and SheetJS's formatted text only: no styles, no editing
 * (every cell is locked), no recalculation. The engine path is the real one.
 */
import type { SheetCell, SheetData, SheetViewModel } from './types';

type XLSXModule = typeof import('xlsx');

export async function sheetFromFile(url: string, documentId: string): Promise<SheetViewModel> {
	const res = await fetch(url);
	if (!res.ok) throw new Error(String(res.status));
	const XLSX: XLSXModule = await import('xlsx');
	const wb = XLSX.read(await res.arrayBuffer(), { type: 'array', cellFormula: true, cellNF: true, cellStyles: true });
	const meta = wb.Workbook?.Sheets ?? [];
	const sheets: SheetData[] = wb.SheetNames.map((name, i) => {
		const ws = wb.Sheets[name];
		const range = ws['!ref'] ? XLSX.utils.decode_range(ws['!ref']) : { s: { r: 0, c: 0 }, e: { r: 0, c: 0 } };
		const cells: SheetCell[] = [];
		for (const key of Object.keys(ws)) {
			if (key.startsWith('!')) continue;
			const at = XLSX.utils.decode_cell(key);
			const cell = ws[key];
			// Errors carry their text as the value, as the engine sends them.
			const value = cell.t === 'e' ? (cell.w ?? '#ERROR!') : (cell.v ?? null);
			cells.push({
				r: at.r + 1,
				c: at.c + 1,
				display: cell.w ?? (value == null ? '' : String(value)),
				value: value instanceof Date ? value.toISOString() : value,
				formula: cell.f ? `=${cell.f}` : null,
				style: null,
				editable: false
			});
		}
		const colWidths: Record<string, number> = {};
		(ws['!cols'] ?? []).forEach((col, c) => {
			// Sizes in px, like the engine's: 7px per character, points × 4/3.
			const chars = col?.width ?? col?.wch;
			const w = col?.hidden ? 0 : chars != null ? chars * 7 : undefined;
			if (w != null) colWidths[XLSX.utils.encode_col(c)] = Math.round(w);
		});
		const rowHeights: Record<string, number> = {};
		(ws['!rows'] ?? []).forEach((row, r) => {
			const h = row?.hidden ? 0 : (row?.hpx ?? (row?.hpt != null ? (row.hpt * 4) / 3 : undefined));
			if (h != null) rowHeights[String(r + 1)] = Math.round(h);
		});
		return {
			name,
			hidden: (meta[i]?.Hidden ?? 0) !== 0,
			dims: { rows: range.e.r + 1, cols: range.e.c + 1 },
			colWidths,
			rowHeights,
			merges: (ws['!merges'] ?? []).map((m) => XLSX.utils.encode_range(m)),
			cells
		};
	});
	return { documentId, version: 0, session: '', sheets, styles: [], names: {} };
}
