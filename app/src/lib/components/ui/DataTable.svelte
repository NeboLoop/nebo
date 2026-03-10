<script lang="ts">
	interface Column {
		key: string;
		label: string;
		sortable?: boolean;
		type?: 'text' | 'number' | 'currency' | 'percentage' | 'date';
		width?: string;
		align?: 'left' | 'center' | 'right';
		format?: (value: any) => string;
	}

	interface Props {
		columns: Column[];
		data: Record<string, any>[];
		loading?: boolean;
		error?: string;
		sortKey?: string;
		sortOrder?: 'asc' | 'desc';
		pageSize?: number;
		showPagination?: boolean;
		zebra?: boolean;
		size?: 'xs' | 'sm' | 'md' | 'lg';
		class?: string;
	}

	let {
		columns,
		data,
		loading = false,
		error,
		sortKey = $bindable(),
		sortOrder = $bindable('asc'),
		pageSize = 10,
		showPagination = true,
		zebra = true,
		size = 'md',
		class: className = ''
	}: Props = $props();

	let currentPage = $state(1);

	// Sort data
	const sortedData = $derived.by(() => {
		if (!sortKey) return data;

		const key = sortKey;
		return [...data].sort((a, b) => {
			const aVal = a[key];
			const bVal = b[key];

			if (aVal == null && bVal == null) return 0;
			if (aVal == null) return 1;
			if (bVal == null) return -1;

			const modifier = sortOrder === 'desc' ? -1 : 1;

			if (typeof aVal === 'number' && typeof bVal === 'number') {
				return (aVal - bVal) * modifier;
			}

			return String(aVal).localeCompare(String(bVal)) * modifier;
		});
	});

	// Paginate data
	const paginatedData = $derived.by(() => {
		if (!showPagination) return sortedData;
		const start = (currentPage - 1) * pageSize;
		const end = start + pageSize;
		return sortedData.slice(start, end);
	});

	const totalPages = $derived(Math.ceil(sortedData.length / pageSize));

	function handleSort(columnKey: string) {
		const column = columns.find(col => col.key === columnKey);
		if (!column?.sortable) return;

		if (sortKey === columnKey) {
			sortOrder = sortOrder === 'asc' ? 'desc' : 'asc';
		} else {
			sortKey = columnKey;
			sortOrder = 'asc';
		}
	}

	function formatCellValue(value: any, column: Column): string {
		if (value == null) return '-';

		if (column.format) {
			return column.format(value);
		}

		switch (column.type) {
			case 'currency':
				return new Intl.NumberFormat('en-US', {
					style: 'currency',
					currency: 'USD'
				}).format(value);
			case 'percentage':
				return `${(value * 100).toFixed(2)}%`;
			case 'number':
				return new Intl.NumberFormat('en-US').format(value);
			case 'date':
				return new Date(value).toLocaleDateString();
			default:
				return String(value);
		}
	}

	function getColumnAlignment(column: Column): string {
		const align = column.align || (column.type === 'number' || column.type === 'currency' ? 'right' : 'left');
		switch (align) {
			case 'center': return 'text-center';
			case 'right': return 'text-right';
			default: return 'text-left';
		}
	}

	function goToPage(page: number) {
		currentPage = Math.max(1, Math.min(page, totalPages));
	}

	const sizeClasses: Record<string, string> = {
		xs: 'table-xs',
		sm: 'table-sm',
		md: '',
		lg: 'table-lg'
	};

	let tableClass = $derived(
		`table ${sizeClasses[size]} ${zebra ? 'table-zebra' : ''} ${className}`.trim()
	);
</script>

<div class="overflow-x-auto rounded-lg bg-base-200">
	{#if loading}
		<div class="flex items-center justify-center p-12">
			<span class="loading loading-spinner loading-md"></span>
			<span class="ml-2 text-sm text-base-content/60">Loading...</span>
		</div>
	{:else if error}
		<div class="flex items-center justify-center p-12">
			<span class="text-sm text-error">Error: {error}</span>
		</div>
	{:else if data.length === 0}
		<div class="flex items-center justify-center p-12">
			<span class="text-sm text-base-content/60">No data available</span>
		</div>
	{:else}
		<table class={tableClass}>
			<thead>
				<tr>
					{#each columns as column}
						<th
							class="{getColumnAlignment(column)} {column.sortable ? 'cursor-pointer hover:bg-base-300' : ''}"
							onclick={() => column.sortable && handleSort(column.key)}
						>
							<div class="flex items-center gap-2 {column.align === 'right' ? 'justify-end' : column.align === 'center' ? 'justify-center' : 'justify-start'}">
								{column.label}
								{#if column.sortable}
									<span>
										{#if sortKey === column.key}
											{#if sortOrder === 'asc'}
												<svg class="h-3 w-3" fill="currentColor" viewBox="0 0 20 20">
													<path fill-rule="evenodd" d="M14.707 12.707a1 1 0 01-1.414 0L10 9.414l-3.293 3.293a1 1 0 01-1.414-1.414l4-4a1 1 0 011.414 0l4 4a1 1 0 010 1.414z" clip-rule="evenodd" />
												</svg>
											{:else}
												<svg class="h-3 w-3" fill="currentColor" viewBox="0 0 20 20">
													<path fill-rule="evenodd" d="M5.293 7.293a1 1 0 011.414 0L10 10.586l3.293-3.293a1 1 0 111.414 1.414l-4 4a1 1 0 01-1.414 0l-4-4a1 1 0 010-1.414z" clip-rule="evenodd" />
												</svg>
											{/if}
										{:else}
											<svg class="h-3 w-3 opacity-30" fill="currentColor" viewBox="0 0 20 20">
												<path d="M5 12a1 1 0 102 0V6.414l1.293 1.293a1 1 0 001.414-1.414l-3-3a1 1 0 00-1.414 0l-3 3a1 1 0 001.414 1.414L5 6.414V12zM15 8a1 1 0 10-2 0v5.586l-1.293-1.293a1 1 0 00-1.414 1.414l3 3a1 1 0 001.414 0l3-3a1 1 0 00-1.414-1.414L15 13.586V8z" />
											</svg>
										{/if}
									</span>
								{/if}
							</div>
						</th>
					{/each}
				</tr>
			</thead>
			<tbody>
				{#each paginatedData as row}
					<tr class="hover">
						{#each columns as column}
							<td class={getColumnAlignment(column)}>
								{formatCellValue(row[column.key], column)}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>

		{#if showPagination && totalPages > 1}
			<div class="flex items-center justify-between border-t border-base-300 px-4 py-3">
				<div class="text-sm text-base-content/60">
					Showing
					<span class="font-medium">{(currentPage - 1) * pageSize + 1}</span>
					to
					<span class="font-medium">{Math.min(currentPage * pageSize, sortedData.length)}</span>
					of
					<span class="font-medium">{sortedData.length}</span>
					results
				</div>
				<div class="join">
					<button
						onclick={() => goToPage(currentPage - 1)}
						disabled={currentPage === 1}
						class="join-item btn btn-sm"
						aria-label="Previous page"
					>
						<svg class="h-4 w-4" fill="currentColor" viewBox="0 0 20 20">
							<path fill-rule="evenodd" d="M12.707 5.293a1 1 0 010 1.414L9.414 10l3.293 3.293a1 1 0 01-1.414 1.414l-4-4a1 1 0 010-1.414l4-4a1 1 0 011.414 0z" clip-rule="evenodd" />
						</svg>
					</button>

					{#each Array.from({length: totalPages}, (_, i) => i + 1) as page}
						{#if page === currentPage}
							<button class="join-item btn btn-sm btn-active">
								{page}
							</button>
						{:else if Math.abs(page - currentPage) <= 2 || page === 1 || page === totalPages}
							<button
								onclick={() => goToPage(page)}
								class="join-item btn btn-sm"
							>
								{page}
							</button>
						{:else if Math.abs(page - currentPage) === 3}
							<span class="join-item btn btn-sm btn-disabled">...</span>
						{/if}
					{/each}

					<button
						onclick={() => goToPage(currentPage + 1)}
						disabled={currentPage === totalPages}
						class="join-item btn btn-sm"
						aria-label="Next page"
					>
						<svg class="h-4 w-4" fill="currentColor" viewBox="0 0 20 20">
							<path fill-rule="evenodd" d="M7.293 14.707a1 1 0 010-1.414L10.586 10 7.293 6.707a1 1 0 011.414-1.414l4 4a1 1 0 010 1.414l-4 4a1 1 0 01-1.414 0z" clip-rule="evenodd" />
						</svg>
					</button>
				</div>
			</div>
		{/if}
	{/if}
</div>
