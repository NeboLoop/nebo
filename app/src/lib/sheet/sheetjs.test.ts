import { afterEach, describe, expect, it, vi } from 'vitest';
import * as XLSX from 'xlsx';
import { sheetFromFile } from './sheetjs';
import { WorkbookModel } from './model';

describe('read-only fallback for files opened by path', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('builds the same view model, every cell locked', async () => {
		const ws = XLSX.utils.aoa_to_sheet([
			['Plan', 'Price'],
			['Pro', 30]
		]);
		ws['!merges'] = [XLSX.utils.decode_range('A3:B3')];
		ws['!cols'] = [{ wch: 20 }];
		const wb = XLSX.utils.book_new();
		XLSX.utils.book_append_sheet(wb, ws, 'Prices');
		const bytes: ArrayBuffer = XLSX.write(wb, { type: 'array', bookType: 'xlsx' });
		vi.stubGlobal('fetch', async () => new Response(bytes));

		const vm = await sheetFromFile('/api/v1/files/prices.xlsx', 'path:/api/v1/files/prices.xlsx');
		const book = new WorkbookModel(vm);
		const s = book.sheet('Prices')!;
		expect(s.get(2, 2)).toMatchObject({ display: '30', value: 30, editable: false });
		expect(s.merges).toEqual([{ r1: 3, c1: 1, r2: 3, c2: 2 }]);
		expect(s.cols.size(1)).toBe(Math.round(20.83203125 * 7)); // the file's width × 7, as the engine sizes it
		expect(s.isEditable(2, 2)).toBe(false);
	});
});
