<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { Brain, Trash2, Edit2, RefreshCw, X, Save, Tag, Eye, Search } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { MemoryItem, MemoryStatsResponse } from '$lib/api/nebo';

	let memories = $state<MemoryItem[]>([]);
	let stats = $state<MemoryStatsResponse | null>(null);
	let isLoading = $state(true);
	let searchQuery = $state('');
	let selectedNamespace = $state('');
	let selectedMemory = $state<MemoryItem | null>(null);
	let isEditing = $state(false);
	let editValue = $state('');
	let editTags = $state('');
	let currentPage = $state(1);
	let total = $state(0);
	let detailOpen = $state(false);
	const pageSize = 50;

	onMount(async () => {
		await Promise.all([loadMemories(), loadStats()]);
	});

	async function loadMemories() {
		isLoading = true;
		try {
			const params: Record<string, any> = {
				limit: pageSize,
				offset: (currentPage - 1) * pageSize
			};
			if (selectedNamespace) {
				params.namespace = selectedNamespace;
			}
			const data = await api.listMemories(params as any);
			memories = data.memories || [];
			total = data.total || 0;
		} catch (error) {
			console.error('Failed to load memories:', error);
		} finally {
			isLoading = false;
		}
	}

	async function loadStats() {
		try {
			stats = await api.getMemoryStats();
		} catch (error) {
			console.error('Failed to load stats:', error);
		}
	}

	async function searchMemoriesHandler() {
		if (!searchQuery.trim()) {
			await loadMemories();
			return;
		}
		isLoading = true;
		try {
			const data = await api.searchMemories({
				query: searchQuery,
				page: currentPage,
				pageSize
			});
			memories = data.memories || [];
			total = data.total || 0;
		} catch (error) {
			console.error('Failed to search memories:', error);
		} finally {
			isLoading = false;
		}
	}

	async function selectMemory(memory: MemoryItem) {
		selectedMemory = memory;
		isEditing = false;
		detailOpen = true;
		try {
			const data = await api.getMemory(String(memory.id));
			selectedMemory = data.memory;
		} catch (error) {
			console.error('Failed to get memory:', error);
		}
	}

	function closeDetail() {
		detailOpen = false;
		selectedMemory = null;
		isEditing = false;
	}

	function startEdit() {
		if (!selectedMemory) return;
		editValue = selectedMemory.value;
		editTags = selectedMemory.tags?.join(', ') || '';
		isEditing = true;
	}

	async function saveEdit() {
		if (!selectedMemory) return;
		try {
			const tags = editTags.split(',').map(t => t.trim()).filter(t => t);
			const data = await api.updateMemory({ value: editValue, tags }, String(selectedMemory.id));
			selectedMemory = data.memory;
			isEditing = false;
			await loadMemories();
		} catch (error) {
			console.error('Failed to update memory:', error);
		}
	}

	async function deleteMemoryHandler(memory: MemoryItem) {
		if (!confirm($t('settingsMemories.deleteConfirm', { values: { key: memory.key } }))) return;
		try {
			await api.deleteMemory(String(memory.id));
			memories = memories.filter(m => m.id !== memory.id);
			if (selectedMemory?.id === memory.id) {
				closeDetail();
			}
			await loadStats();
		} catch (error) {
			console.error('Failed to delete memory:', error);
		}
	}

	function selectNamespaceFilter(layer: string) {
		selectedNamespace = layer === selectedNamespace ? '' : layer;
		currentPage = 1;
		loadMemories();
	}

	function getLayerFromNamespace(namespace: string): string {
		if (namespace.startsWith('tacit/') || namespace === 'tacit') return 'tacit';
		if (namespace.startsWith('daily/') || namespace === 'daily') return 'daily';
		if (namespace.startsWith('entity/') || namespace === 'entity') return 'entity';
		return 'other';
	}

	function getLayerColor(layer: string): string {
		switch (layer) {
			case 'tacit': return 'bg-primary/10 text-primary';
			case 'daily': return 'bg-secondary/10 text-secondary';
			case 'entity': return 'bg-accent/10 text-accent';
			default: return 'bg-base-content/5 text-base-content/90';
		}
	}

	function formatDate(dateStr: string | number): string {
		if (!dateStr) return $t('time.never');
		const d = new Date(dateStr);
		if (isNaN(d.getTime())) return $t('time.never');
		return d.toLocaleString();
	}

	function formatShortDate(dateStr: string | number): string {
		if (!dateStr) return '';
		const d = new Date(dateStr);
		if (isNaN(d.getTime())) return '';
		return d.toLocaleDateString([], { month: 'short', day: 'numeric' });
	}

	/** Derive unique layer names from namespaces for filter pills */
	function getLayerFilters(namespaces: string[]): string[] {
		const layers = new Set<string>();
		for (const ns of namespaces) {
			layers.add(getLayerFromNamespace(ns));
		}
		return [...layers].sort();
	}

	function handleSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			searchMemoriesHandler();
		}
	}

	function handleSearchClear() {
		searchQuery = '';
		currentPage = 1;
		loadMemories();
	}
</script>

<!-- Header -->
<div class="mb-4 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsMemories.title')}</h2>
		{#if stats}
			<p class="text-base text-base-content/80">
				<span class="font-medium text-base-content/90">{stats.totalCount}</span> total
				<span class="mx-1">&middot;</span>
				<span class="text-primary font-medium">{stats.layerCounts?.tacit || 0}</span> tacit
				<span class="mx-1">&middot;</span>
				<span class="text-secondary font-medium">{stats.layerCounts?.daily || 0}</span> daily
				<span class="mx-1">&middot;</span>
				<span class="text-accent font-medium">{stats.layerCounts?.entity || 0}</span> entity
			</p>
		{:else}
			<p class="text-base text-base-content/80">{$t('settingsMemories.description')}</p>
		{/if}
	</div>
	<button
		type="button"
		class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-1.5"
		onclick={() => { loadMemories(); loadStats(); }}
	>
		<RefreshCw class="w-3.5 h-3.5" />
	</button>
</div>

<!-- Layer filter pills -->
{#if stats?.namespaces?.length}
	{@const layers = getLayerFilters(stats.namespaces)}
	<div class="flex flex-wrap gap-1.5 mb-4">
		<button
			type="button"
			class="px-2.5 py-1 rounded-lg text-sm font-medium transition-colors {selectedNamespace === '' ? 'bg-primary/10 text-primary border border-primary/30' : 'bg-base-content/5 text-base-content/60 border border-transparent hover:border-base-content/15'}"
			onclick={() => selectNamespaceFilter('')}
		>
			{$t('settingsMemories.all')}
		</button>
		{#each layers as layer}
			<button
				type="button"
				class="px-2.5 py-1 rounded-lg text-sm font-medium transition-colors {selectedNamespace === layer ? getLayerColor(layer) + ' border border-current/30' : 'bg-base-content/5 text-base-content/60 border border-transparent hover:border-base-content/15'}"
				onclick={() => selectNamespaceFilter(layer)}
			>
				{layer}
				{#if stats.layerCounts?.[layer]}
					<span class="ml-1 text-xs opacity-70">{stats.layerCounts[layer]}</span>
				{/if}
			</button>
		{/each}
	</div>
{/if}

<!-- Search -->
<div class="relative mb-4">
	<Search class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-base-content/60" />
	<input
		type="text"
		class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 pl-10 pr-10 text-base focus:outline-none focus:border-primary/50 transition-colors"
		placeholder={$t('settingsMemories.searchPlaceholder')}
		bind:value={searchQuery}
		onkeydown={handleSearchKeydown}
	/>
	{#if searchQuery}
		<button
			type="button"
			class="absolute right-3 top-1/2 -translate-y-1/2 p-0.5 text-base-content/60 hover:text-base-content transition-colors"
			onclick={handleSearchClear}
		>
			<X class="w-4 h-4" />
		</button>
	{/if}
</div>

<!-- Memory List -->
{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsMemories.loadingMemories')}</span>
	</div>
{:else if memories.length === 0}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-12 text-center">
		<Brain class="w-10 h-10 mx-auto mb-3 text-base-content/60" />
		<p class="text-base text-base-content/80">{$t('settingsMemories.noMemories')}</p>
	</div>
{:else}
	<div class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10 max-h-[60vh] overflow-y-auto">
		{#each memories as memory}
			<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
			<div
				class="w-full p-4 text-left transition-colors group hover:bg-base-content/5 cursor-pointer {selectedMemory?.id === memory.id ? 'bg-primary/5' : ''}"
				onclick={() => selectMemory(memory)}
				role="button"
				tabindex="0"
			>
				<div class="flex items-start justify-between gap-2 mb-1">
					<span class="text-base font-medium text-base-content truncate flex-1">{memory.key}</span>
					<div class="flex items-center gap-1.5 shrink-0">
						<button
							type="button"
							class="p-1 rounded-lg opacity-0 group-hover:opacity-100 transition-opacity text-base-content/60 hover:text-error hover:bg-error/10"
							onclick={(e) => { e.stopPropagation(); deleteMemoryHandler(memory); }}
						>
							<Trash2 class="w-3.5 h-3.5" />
						</button>
						<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded {getLayerColor(getLayerFromNamespace(memory.namespace))}">
							{getLayerFromNamespace(memory.namespace)}
						</span>
					</div>
				</div>
				<p class="text-base text-base-content/80 line-clamp-2 mb-2">{memory.value}</p>
				<div class="flex items-center gap-3 text-sm text-base-content/80">
					<span class="truncate">{memory.namespace}</span>
					{#if memory.createdAt}
						<span>{formatShortDate(memory.createdAt)}</span>
					{/if}
					<span class="flex items-center gap-1">
						<Eye class="w-3 h-3" />
						{memory.accessCount}
					</span>
				</div>
			</div>
		{/each}
	</div>

	<!-- Pagination -->
	{#if total > pageSize}
		<div class="flex items-center justify-center gap-3 mt-4">
			<button
				type="button"
				class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 transition-colors disabled:opacity-30"
				disabled={currentPage === 1}
				onclick={() => { currentPage--; loadMemories(); }}
			>
				{$t('common.previous')}
			</button>
			<span class="text-base text-base-content/80">
				{$t('settingsMemories.pageOf', { values: { current: currentPage, total: Math.ceil(total / pageSize) } })}
			</span>
			<button
				type="button"
				class="h-8 px-3 rounded-lg bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/60 hover:border-base-content/40 transition-colors disabled:opacity-30"
				disabled={currentPage >= Math.ceil(total / pageSize)}
				onclick={() => { currentPage++; loadMemories(); }}
			>
				{$t('common.next')}
			</button>
		</div>
	{/if}
{/if}

<!-- Memory Detail Modal -->
<Modal bind:show={detailOpen} title={selectedMemory?.key || 'Memory Detail'} size="md" onclose={closeDetail}>
	{#if selectedMemory}
		<!-- Metadata -->
		<div class="space-y-3 mb-5">
			<div class="grid grid-cols-2 gap-3">
				<div>
					<p class="text-sm text-base-content/80 mb-0.5">{$t('settingsMemories.namespace')}</p>
					<p class="text-base text-base-content">{selectedMemory.namespace}</p>
				</div>
				<div>
					<p class="text-sm text-base-content/80 mb-0.5">{$t('settingsMemories.layer')}</p>
					<span class="text-sm font-semibold uppercase px-1.5 py-0.5 rounded {getLayerColor(getLayerFromNamespace(selectedMemory.namespace))}">
						{getLayerFromNamespace(selectedMemory.namespace)}
					</span>
				</div>
				<div>
					<p class="text-sm text-base-content/80 mb-0.5">{$t('settingsMemories.accessCount')}</p>
					<p class="text-base text-base-content">{selectedMemory.accessCount}</p>
				</div>
				<div>
					<p class="text-sm text-base-content/80 mb-0.5">{$t('settingsMemories.lastAccess')}</p>
					<p class="text-base text-base-content/80">{selectedMemory.accessedAt ? formatDate(selectedMemory.accessedAt) : $t('time.never')}</p>
				</div>
				<div>
					<p class="text-sm text-base-content/80 mb-0.5">{$t('settingsMemories.created')}</p>
					<p class="text-base text-base-content/80">{formatDate(selectedMemory.createdAt)}</p>
				</div>
				<div>
					<p class="text-sm text-base-content/80 mb-0.5">{$t('settingsMemories.updated')}</p>
					<p class="text-base text-base-content/80">{formatDate(selectedMemory.updatedAt)}</p>
				</div>
			</div>
		</div>

		<!-- Tags -->
		{#if isEditing}
			<div class="mb-5">
				<label class="text-base font-medium text-base-content/80 mb-1.5 block" for="edit-tags">{$t('settingsMemories.tagsLabel')}</label>
				<input
					id="edit-tags"
					type="text"
					class="w-full h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
					bind:value={editTags}
					placeholder={$t('settingsMemories.tagsPlaceholder')}
				/>
			</div>
		{:else if selectedMemory.tags?.length}
			<div class="mb-5">
				<p class="text-sm text-base-content/80 mb-1.5">{$t('settingsMemories.tags')}</p>
				<div class="flex flex-wrap gap-1.5">
					{#each selectedMemory.tags as tag}
						<span class="inline-flex items-center gap-1 text-sm font-medium bg-base-content/5 text-base-content/60 px-2 py-0.5 rounded">
							<Tag class="w-3 h-3" />
							{tag}
						</span>
					{/each}
				</div>
			</div>
		{/if}

		<!-- Value -->
		<div>
			<p class="text-sm text-base-content/80 mb-1.5">{$t('settingsMemories.value')}</p>
			{#if isEditing}
				<textarea
					class="w-full rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base focus:outline-none focus:border-primary/50 transition-colors resize-none h-48"
					bind:value={editValue}
				></textarea>
			{:else}
				<div class="rounded-xl bg-base-content/5 border border-base-content/10 p-4 whitespace-pre-wrap text-base text-base-content/80 max-h-64 overflow-y-auto">
					{selectedMemory.value}
				</div>
			{/if}
		</div>
	{/if}

	{#snippet footer()}
		<div class="flex items-center justify-between w-full">
			{#if isEditing}
				<button
					type="button"
					class="h-9 px-4 rounded-full text-base font-medium text-base-content/80 hover:bg-base-content/5 transition-colors"
					onclick={() => isEditing = false}
				>
					{$t('common.cancel')}
				</button>
				<button
					type="button"
					class="h-9 px-5 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all flex items-center gap-1.5"
					onclick={saveEdit}
				>
					<Save class="w-4 h-4" />
					{$t('common.save')}
				</button>
			{:else}
				<div class="flex items-center gap-2">
					<button
						type="button"
						class="h-8 px-3 rounded-lg text-sm font-medium text-base-content/60 hover:bg-base-content/5 transition-colors flex items-center gap-1.5"
						onclick={startEdit}
					>
						<Edit2 class="w-3.5 h-3.5" />
						{$t('common.edit')}
					</button>
					<button
						type="button"
						class="h-8 px-3 rounded-lg text-sm font-medium text-base-content/60 hover:text-error hover:bg-error/10 transition-colors flex items-center gap-1.5"
						onclick={() => deleteMemoryHandler(selectedMemory!)}
					>
						<Trash2 class="w-3.5 h-3.5" />
						{$t('common.delete')}
					</button>
				</div>
				<button
					type="button"
					class="h-9 px-4 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
					onclick={closeDetail}
				>
					{$t('common.close')}
				</button>
			{/if}
		</div>
	{/snippet}
</Modal>
