/**
 * The spreadsheet view model: the contract between the sheet engine
 * (crates/sheet, GET /api/v1/work/{documentId}/sheet) and the grid.
 *
 * The backend owns ALL formatting and calculation. The grid renders `display`
 * as given and never formats or computes a cell. Rows and columns are
 * 1-based (r=1,c=1 is A1).
 */

export interface SheetCell {
	r: number;
	c: number;
	/** The formatted text the grid shows ("$30", "12.5%"). */
	display: string;
	/** The raw value: a number, string, boolean, or null for an empty cell. An error is its text ("#DIV/0!"). */
	value: number | string | boolean | null;
	/** "=B16*C16", or null for a constant. */
	formula: string | null;
	/** Index into SheetViewModel.styles, or null/undefined for the default style. */
	style?: number | null;
	/** True only when the cell's style is unlocked: the one place input is accepted. */
	editable: boolean;
}

export interface SheetChart {
	type: string;
	title?: string;
	anchor: string;
	series: { name?: string; ref: string }[];
}

export interface SheetData {
	name: string;
	tabColor?: string | null;
	hidden?: boolean;
	protected?: boolean;
	dims: { rows: number; cols: number };
	freeze?: { rows: number; cols: number } | null;
	/** Column letter → width in CSS px (the engine converts Excel's units). */
	colWidths?: Record<string, number>;
	/** 1-based row number → height in CSS px. */
	rowHeights?: Record<string, number>;
	/** A1 ranges, e.g. "A1:E1". */
	merges?: string[];
	cells: SheetCell[];
	charts?: SheetChart[];
}

export interface CellStyle {
	bold?: boolean;
	italic?: boolean;
	underline?: boolean;
	/** "#RRGGBB" fill from the file. */
	fill?: string | null;
	/** "#RRGGBB" font colour from the file. */
	color?: string | null;
	align?: 'left' | 'center' | 'right' | 'general' | string | null;
	valign?: 'top' | 'center' | 'bottom' | string | null;
	/** Font size in points. */
	size?: number | null;
	numFmt?: string | null;
	wrap?: boolean;
	/** Cell borders (not drawn yet; gridlines are). */
	border?: Record<'top' | 'right' | 'bottom' | 'left', { style: string; color?: string } | undefined> | null;
}

export interface SheetViewModel {
	documentId: string;
	version: number;
	/** Edit session id. Optional in the contract; the client mints one when absent. */
	session?: string;
	sheets: SheetData[];
	styles: CellStyle[];
	/** Defined names → "Sheet!$B$9". */
	names?: Record<string, string>;
}

export interface SheetEdit {
	sheet: string;
	cell: string;
	input: string;
}

export interface ChangedCell {
	sheet: string;
	cell: string;
	display: string;
	value: number | string | boolean | null;
	formula?: string | null;
}

/** An edit the engine refused (a locked cell, a bad formula). */
export interface EditError {
	sheet?: string;
	cell?: string;
	error?: string;
	message?: string;
}

export interface EditResponse {
	changed: ChangedCell[];
	errors: (EditError | string)[];
}

export interface SaveResponse {
	version: number;
}
