<script lang="ts">
	import { onMount } from 'svelte';
	import {
		ChevronUp,
		ChevronDown,
		ChevronsUpDown,
		Search,
		Download,
		Settings2,
		ChevronLeft,
		ChevronRight,
		ChevronsLeft,
		ChevronsRight,
		X,
		Check,
		Filter,
		ArrowUpDown
	} from 'lucide-svelte';
	import type { TableColumn, TableRow } from './types';

	interface Props {
		columns: TableColumn[];
		rows: TableRow[];
		height?: number;
		striped?: boolean;
		hoverable?: boolean;
		paginate?: boolean;
		pageSize?: number;
		searchable?: boolean;
		exportable?: boolean;
		selectable?: boolean;
		resizable?: boolean;
		showColumnToggle?: boolean;
		stickyHeader?: boolean;
		compactMode?: boolean;
		onRowClick?: (row: TableRow, index: number) => void;
		onSelectionChange?: (selectedRows: TableRow[]) => void;
	}

	let {
		columns: initialColumns = [],
		rows = [],
		height,
		striped = true,
		hoverable = true,
		paginate = true,
		pageSize: initialPageSize = 10,
		searchable = true,
		exportable = true,
		selectable = false,
		resizable = true,
		showColumnToggle = true,
		stickyHeader = true,
		compactMode = false,
		onRowClick,
		onSelectionChange
	}: Props = $props();

	// State
	let sortKey = $state<string | null>(null);
	let sortDirection = $state<'asc' | 'desc'>('asc');
	let currentPage = $state(0);
	let pageSize = $state(10);
	let searchQuery = $state('');
	let columnFilters = $state<Record<string, string>>({});
	let visibleColumns = $state<Set<string>>(new Set());
	let selectedRows = $state<Set<number>>(new Set());

	// Sync state with props when they change
	$effect(() => {
		pageSize = initialPageSize;
	});

	$effect(() => {
		visibleColumns = new Set(initialColumns.map((c) => c.key));
	});
	let columnWidths = $state<Record<string, number>>({});
	let showColumnMenu = $state(false);
	let showFilterMenu = $state<string | null>(null);
	let resizingColumn = $state<string | null>(null);
	let resizeStartX = $state(0);
	let resizeStartWidth = $state(0);

	// Refs
	let containerRef: HTMLDivElement;
	let tableRef: HTMLTableElement;

	// Computed columns with visibility
	const columns = $derived(initialColumns.filter((c) => visibleColumns.has(c.key)));

	// Filter and search rows
	const filteredRows = $derived.by(() => {
		let result = [...rows];

		// Apply global search
		if (searchQuery.trim()) {
			const query = searchQuery.toLowerCase();
			result = result.filter((row) =>
				columns.some((col) => {
					const val = row[col.key];
					return val !== null && val !== undefined && String(val).toLowerCase().includes(query);
				})
			);
		}

		// Apply column filters
		for (const [key, filterValue] of Object.entries(columnFilters)) {
			if (filterValue.trim()) {
				const query = filterValue.toLowerCase();
				result = result.filter((row) => {
					const val = row[key];
					return val !== null && val !== undefined && String(val).toLowerCase().includes(query);
				});
			}
		}

		return result;
	});

	// Sort rows
	const sortedRows = $derived.by(() => {
		let sorted = [...filteredRows];

		if (sortKey) {
			const key = sortKey; // Capture for type narrowing
			sorted.sort((a, b) => {
				const aVal = a[key];
				const bVal = b[key];

				if (aVal === bVal) return 0;
				if (aVal === null || aVal === undefined) return 1;
				if (bVal === null || bVal === undefined) return -1;

				let comparison = 0;
				if (typeof aVal === 'number' && typeof bVal === 'number') {
					comparison = aVal - bVal;
				} else if (aVal instanceof Date && bVal instanceof Date) {
					comparison = aVal.getTime() - bVal.getTime();
				} else {
					comparison = String(aVal).localeCompare(String(bVal));
				}

				return sortDirection === 'asc' ? comparison : -comparison;
			});
		}

		return sorted;
	});

	// Paginate rows
	const paginatedRows = $derived.by(() => {
		if (!paginate) return sortedRows;
		const start = currentPage * pageSize;
		const end = start + pageSize;
		return sortedRows.slice(start, end);
	});

	const totalPages = $derived(Math.ceil(filteredRows.length / pageSize));
	const totalRows = $derived(filteredRows.length);
	const startRow = $derived(currentPage * pageSize + 1);
	const endRow = $derived(Math.min((currentPage + 1) * pageSize, totalRows));

	// Selection helpers
	const allSelected = $derived(paginatedRows.length > 0 && paginatedRows.every((_, i) => selectedRows.has(currentPage * pageSize + i)));
	const someSelected = $derived(paginatedRows.some((_, i) => selectedRows.has(currentPage * pageSize + i)));

	function handleSort(key: string, sortable: boolean = true) {
		if (!sortable) return;

		if (sortKey === key) {
			sortDirection = sortDirection === 'asc' ? 'desc' : 'asc';
		} else {
			sortKey = key;
			sortDirection = 'asc';
		}
		currentPage = 0;
	}

	function goToPage(page: number) {
		currentPage = Math.max(0, Math.min(page, totalPages - 1));
	}

	function formatCell(value: any, column: TableColumn): string {
		if (column.format) {
			return column.format(value);
		}
		if (value === null || value === undefined) {
			return '—';
		}
		if (typeof value === 'number') {
			// Auto-format numbers
			if (Math.abs(value) >= 1000000) {
				return (value / 1000000).toFixed(1) + 'M';
			}
			if (Math.abs(value) >= 1000) {
				return (value / 1000).toFixed(1) + 'K';
			}
			if (!Number.isInteger(value)) {
				return value.toFixed(2);
			}
		}
		if (value instanceof Date) {
			return value.toLocaleDateString();
		}
		return String(value);
	}

	function getCellClass(value: any, column: TableColumn): string {
		if (typeof value === 'number') {
			if (value > 0) return 'text-success';
			if (value < 0) return 'text-error';
		}
		return '';
	}

	function toggleColumnVisibility(key: string) {
		const newSet = new Set(visibleColumns);
		if (newSet.has(key)) {
			if (newSet.size > 1) {
				newSet.delete(key);
			}
		} else {
			newSet.add(key);
		}
		visibleColumns = newSet;
	}

	function toggleRowSelection(index: number) {
		const globalIndex = currentPage * pageSize + index;
		const newSet = new Set(selectedRows);
		if (newSet.has(globalIndex)) {
			newSet.delete(globalIndex);
		} else {
			newSet.add(globalIndex);
		}
		selectedRows = newSet;

		if (onSelectionChange) {
			const selected = Array.from(newSet).map((i) => sortedRows[i]).filter(Boolean);
			onSelectionChange(selected);
		}
	}

	function toggleSelectAll() {
		const newSet = new Set(selectedRows);
		const pageIndices = paginatedRows.map((_, i) => currentPage * pageSize + i);

		if (allSelected) {
			pageIndices.forEach((i) => newSet.delete(i));
		} else {
			pageIndices.forEach((i) => newSet.add(i));
		}
		selectedRows = newSet;

		if (onSelectionChange) {
			const selected = Array.from(newSet).map((i) => sortedRows[i]).filter(Boolean);
			onSelectionChange(selected);
		}
	}

	function handleRowClick(row: TableRow, index: number) {
		if (onRowClick) {
			onRowClick(row, currentPage * pageSize + index);
		}
	}

	// Column resizing
	function startResize(e: MouseEvent, columnKey: string) {
		e.preventDefault();
		e.stopPropagation();
		resizingColumn = columnKey;
		resizeStartX = e.clientX;
		resizeStartWidth = columnWidths[columnKey] || 150;

		window.addEventListener('mousemove', handleResize);
		window.addEventListener('mouseup', stopResize);
	}

	function handleResize(e: MouseEvent) {
		if (!resizingColumn) return;
		const diff = e.clientX - resizeStartX;
		const newWidth = Math.max(80, resizeStartWidth + diff);
		columnWidths = { ...columnWidths, [resizingColumn]: newWidth };
	}

	function stopResize() {
		resizingColumn = null;
		window.removeEventListener('mousemove', handleResize);
		window.removeEventListener('mouseup', stopResize);
	}

	// Export functions
	function exportCSV() {
		const headers = columns.map((c) => c.label).join(',');
		const csvRows = sortedRows.map((row) =>
			columns.map((col) => {
				const val = row[col.key];
				if (val === null || val === undefined) return '';
				const str = String(val);
				// Escape quotes and wrap in quotes if contains comma or quote
				if (str.includes(',') || str.includes('"') || str.includes('\n')) {
					return `"${str.replace(/"/g, '""')}"`;
				}
				return str;
			}).join(',')
		);

		const csv = [headers, ...csvRows].join('\n');
		downloadFile(csv, 'table-export.csv', 'text/csv');
	}

	function exportJSON() {
		const data = sortedRows.map((row) => {
			const obj: Record<string, any> = {};
			columns.forEach((col) => {
				obj[col.key] = row[col.key];
			});
			return obj;
		});
		downloadFile(JSON.stringify(data, null, 2), 'table-export.json', 'application/json');
	}

	function downloadFile(content: string, filename: string, mimeType: string) {
		const blob = new Blob([content], { type: mimeType });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = filename;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	function clearFilters() {
		searchQuery = '';
		columnFilters = {};
		currentPage = 0;
	}

	const hasActiveFilters = $derived(searchQuery.trim() !== '' || Object.values(columnFilters).some((v) => v.trim() !== ''));

	// Page size options
	const pageSizeOptions = [10, 25, 50, 100];

	// Pagination range
	const paginationRange = $derived.by(() => {
		const range: (number | 'ellipsis')[] = [];
		const delta = 2;

		for (let i = 0; i < totalPages; i++) {
			if (
				i === 0 ||
				i === totalPages - 1 ||
				(i >= currentPage - delta && i <= currentPage + delta)
			) {
				range.push(i);
			} else if (range[range.length - 1] !== 'ellipsis') {
				range.push('ellipsis');
			}
		}

		return range;
	});
</script>

<div
	bind:this={containerRef}
	class="card overflow-hidden {compactMode ? 'p-2' : ''}"
>
	<!-- Toolbar -->
	{#if searchable || exportable || showColumnToggle}
		<div class="flex items-center gap-3 p-3 border-b border-base-200 bg-base-100/50">
			<!-- Search -->
			{#if searchable}
				<div class="relative flex-1 max-w-xs">
					<Search class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-base-400" />
					<input
						type="text"
						bind:value={searchQuery}
						placeholder="Search all columns..."
						class="input input-sm w-full pl-9 pr-8"
					/>
					{#if searchQuery}
						<button
							class="absolute right-2 top-1/2 -translate-y-1/2 text-base-400 hover:text-base-700"
							onclick={() => (searchQuery = '')}
						>
							<X class="w-4 h-4" />
						</button>
					{/if}
				</div>
			{/if}

			<!-- Results count -->
			<div class="text-xs text-base-500">
				{totalRows} {totalRows === 1 ? 'row' : 'rows'}
				{#if hasActiveFilters}
					<span class="text-primary">(filtered)</span>
				{/if}
			</div>

			<div class="flex-1"></div>

			<!-- Clear filters -->
			{#if hasActiveFilters}
				<button
					class="btn btn-ghost btn-sm"
					onclick={clearFilters}
				>
					<X class="w-3.5 h-3.5" />
					Clear filters
				</button>
			{/if}

			<!-- Column toggle -->
			{#if showColumnToggle}
				<div class="dropdown dropdown-end">
					<button
						class="btn btn-ghost btn-sm"
						onclick={() => (showColumnMenu = !showColumnMenu)}
					>
						<Settings2 class="w-3.5 h-3.5" />
						Columns
					</button>
					{#if showColumnMenu}
						<div class="dropdown-menu mt-1 w-48">
							<div class="p-2 border-b border-base-200 text-xs font-medium text-base-500">
								Toggle Columns
							</div>
							<div class="max-h-64 overflow-y-auto p-1">
								{#each initialColumns as column}
									<button
										class="dropdown-item"
										onclick={() => toggleColumnVisibility(column.key)}
									>
										<div
											class="w-4 h-4 rounded border flex items-center justify-center {visibleColumns.has(column.key) ? 'bg-primary border-primary' : 'border-base-400'}"
										>
											{#if visibleColumns.has(column.key)}
												<Check class="w-3 h-3 text-white" />
											{/if}
										</div>
										{column.label}
									</button>
								{/each}
							</div>
						</div>
					{/if}
				</div>
			{/if}

			<!-- Export -->
			{#if exportable}
				<div class="dropdown dropdown-end group">
					<button class="btn btn-ghost btn-sm">
						<Download class="w-3.5 h-3.5" />
						Export
					</button>
					<div class="dropdown-menu mt-1 w-32 opacity-0 invisible group-hover:opacity-100 group-hover:visible transition-all">
						<button
							class="dropdown-item"
							onclick={exportCSV}
						>
							Export CSV
						</button>
						<button
							class="dropdown-item"
							onclick={exportJSON}
						>
							Export JSON
						</button>
					</div>
				</div>
			{/if}
		</div>
	{/if}

	<!-- Table -->
	<div class="overflow-auto" style={height ? `max-height: ${height}px` : ''}>
		<table bind:this={tableRef} class="table table-zebra w-full">
			<thead class={stickyHeader ? 'sticky top-0 z-10 bg-base-200' : 'bg-base-200'}>
				<tr>
					<!-- Selection checkbox -->
					{#if selectable}
						<th class="w-10 px-3">
							<input
								type="checkbox"
								checked={allSelected}
								indeterminate={someSelected && !allSelected}
								onchange={toggleSelectAll}
								class="checkbox checkbox-sm checkbox-primary"
							/>
						</th>
					{/if}

					{#each columns as column}
						{@const width = columnWidths[column.key]}
						<th
							class="group relative select-none {column.sortable !== false ? 'cursor-pointer hover:bg-base-300' : ''} px-4 py-3 text-left text-xs font-medium text-base-600 uppercase tracking-wider"
							style="text-align: {column.align || 'left'}; {width ? `width: ${width}px; min-width: ${width}px;` : ''}"
							onclick={() => handleSort(column.key, column.sortable !== false)}
						>
							<div class="flex items-center gap-2 {column.align === 'center' ? 'justify-center' : column.align === 'right' ? 'justify-end' : 'justify-start'}">
								<span class="truncate">{column.label}</span>

								<!-- Sort indicator -->
								{#if column.sortable !== false}
									<span class="flex-shrink-0">
										{#if sortKey === column.key}
											{#if sortDirection === 'asc'}
												<ChevronUp class="w-4 h-4 text-primary" />
											{:else}
												<ChevronDown class="w-4 h-4 text-primary" />
											{/if}
										{:else}
											<ChevronsUpDown class="w-4 h-4 opacity-0 group-hover:opacity-50 transition-opacity" />
										{/if}
									</span>
								{/if}

								<!-- Column filter -->
								<button
									class="flex-shrink-0 p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-base-400/20 transition-all {columnFilters[column.key] ? 'opacity-100 text-primary' : 'text-base-500'}"
									onclick={(e) => {
										e.stopPropagation();
										showFilterMenu = showFilterMenu === column.key ? null : column.key;
									}}
								>
									<Filter class="w-3.5 h-3.5" />
								</button>
							</div>

							<!-- Column filter dropdown -->
							{#if showFilterMenu === column.key}
								<div
									class="dropdown-menu absolute left-0 top-full mt-1 w-48 p-2"
									onclick={(e) => e.stopPropagation()}
									onkeydown={(e) => e.stopPropagation()}
									role="dialog"
									aria-label="Filter options"
									tabindex="-1"
								>
									<input
										type="text"
										bind:value={columnFilters[column.key]}
										placeholder="Filter {column.label}..."
										class="input input-sm w-full"
									/>
									{#if columnFilters[column.key]}
										<button
											class="btn btn-ghost btn-xs w-full mt-2"
											onclick={() => {
												columnFilters = { ...columnFilters, [column.key]: '' };
											}}
										>
											Clear filter
										</button>
									{/if}
								</div>
							{/if}

							<!-- Resize handle -->
							{#if resizable}
								<button
									type="button"
									class="absolute right-0 top-0 bottom-0 w-1 cursor-col-resize bg-transparent hover:bg-primary/50 transition-colors border-0 p-0"
									onmousedown={(e) => startResize(e, column.key)}
									aria-label="Resize column"
								></button>
							{/if}
						</th>
					{/each}
				</tr>
			</thead>

			<tbody>
				{#each paginatedRows as row, rowIndex}
					{@const globalIndex = currentPage * pageSize + rowIndex}
					{@const isSelected = selectedRows.has(globalIndex)}
					<tr
						class="transition-colors {hoverable ? 'hover:bg-base-100' : ''} {isSelected ? 'bg-primary/10' : ''} {onRowClick ? 'cursor-pointer' : ''}"
						onclick={() => handleRowClick(row, rowIndex)}
					>
						<!-- Selection checkbox -->
						{#if selectable}
							<td class="w-10 px-3" onclick={(e) => e.stopPropagation()}>
								<input
									type="checkbox"
									checked={isSelected}
									onchange={() => toggleRowSelection(rowIndex)}
									class="checkbox checkbox-sm checkbox-primary"
								/>
							</td>
						{/if}

						{#each columns as column}
							{@const value = row[column.key]}
							{@const width = columnWidths[column.key]}
							<td
								class="px-4 py-3 text-sm {getCellClass(value, column)}"
								style="text-align: {column.align || 'left'}; {width ? `width: ${width}px; min-width: ${width}px;` : ''}"
							>
								<span class="truncate block">{formatCell(value, column)}</span>
							</td>
						{/each}
					</tr>
				{/each}

				{#if paginatedRows.length === 0}
					<tr>
						<td colspan={columns.length + (selectable ? 1 : 0)} class="text-center py-12">
							<div class="flex flex-col items-center justify-center">
								<svg class="w-12 h-12 text-base-400 mb-3" fill="none" viewBox="0 0 24 24" stroke="currentColor">
									<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z" />
								</svg>
								<p class="text-base-600 font-medium mb-1">No data found</p>
								{#if hasActiveFilters}
									<p class="text-base-500 text-sm">Try adjusting your search or filters</p>
								{/if}
							</div>
						</td>
					</tr>
				{/if}
			</tbody>
		</table>
	</div>

	<!-- Footer with pagination -->
	{#if paginate && rows.length > 0}
		<div class="flex items-center justify-between px-4 py-3 border-t border-base-200 bg-base-100/50">
			<!-- Page size selector -->
			<div class="flex items-center gap-2">
				<span class="text-xs text-base-500">Show</span>
				<select
					bind:value={pageSize}
					onchange={() => (currentPage = 0)}
					class="select select-xs select-bordered"
				>
					{#each pageSizeOptions as option}
						<option value={option}>{option}</option>
					{/each}
				</select>
				<span class="text-xs text-base-500">rows</span>
			</div>

			<!-- Results info -->
			<div class="text-xs text-base-500">
				{#if totalRows > 0}
					{startRow}-{endRow} of {totalRows}
				{:else}
					No results
				{/if}
			</div>

			<!-- Pagination controls -->
			<div class="join">
				<button
					class="join-item btn btn-xs"
					disabled={currentPage === 0}
					onclick={() => goToPage(0)}
					title="First page"
				>
					<ChevronsLeft class="w-4 h-4" />
				</button>
				<button
					class="join-item btn btn-xs"
					disabled={currentPage === 0}
					onclick={() => goToPage(currentPage - 1)}
					title="Previous page"
				>
					<ChevronLeft class="w-4 h-4" />
				</button>

				{#each paginationRange as page}
					{#if page === 'ellipsis'}
						<span class="join-item btn btn-xs btn-disabled">...</span>
					{:else}
						<button
							class="join-item btn btn-xs {currentPage === page ? 'btn-primary' : ''}"
							onclick={() => goToPage(page)}
						>
							{page + 1}
						</button>
					{/if}
				{/each}

				<button
					class="join-item btn btn-xs"
					disabled={currentPage >= totalPages - 1}
					onclick={() => goToPage(currentPage + 1)}
					title="Next page"
				>
					<ChevronRight class="w-4 h-4" />
				</button>
				<button
					class="join-item btn btn-xs"
					disabled={currentPage >= totalPages - 1}
					onclick={() => goToPage(totalPages - 1)}
					title="Last page"
				>
					<ChevronsRight class="w-4 h-4" />
				</button>
			</div>
		</div>
	{/if}
</div>

<!-- Click outside handlers -->
<svelte:window
	onclick={() => {
		showColumnMenu = false;
		showFilterMenu = null;
	}}
/>
