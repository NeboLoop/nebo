<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Brain, Search, Trash2, Edit2, RefreshCw, X, Save, FolderTree, Tag, Clock, Eye } from 'lucide-svelte';
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
	const pageSize = 50;

	onMount(async () => {
		await Promise.all([loadMemories(), loadStats()]);
	});

	async function loadMemories() {
		isLoading = true;
		try {
			const params: api.ListMemoriesRequestParams = {
				page: currentPage,
				pageSize
			};
			if (selectedNamespace) {
				params.namespace = selectedNamespace;
			}
			const data = await api.listMemories(params);
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
		try {
			const data = await api.getMemory(String(memory.id));
			selectedMemory = data.memory;
		} catch (error) {
			console.error('Failed to get memory:', error);
		}
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
		if (!confirm(`Delete memory "${memory.key}"?`)) return;
		try {
			await api.deleteMemory(String(memory.id));
			memories = memories.filter(m => m.id !== memory.id);
			if (selectedMemory?.id === memory.id) {
				selectedMemory = null;
			}
			await loadStats();
		} catch (error) {
			console.error('Failed to delete memory:', error);
		}
	}

	function selectNamespaceFilter(ns: string) {
		selectedNamespace = ns === selectedNamespace ? '' : ns;
		currentPage = 1;
		loadMemories();
	}

	function getLayerFromNamespace(namespace: string): string {
		if (namespace.startsWith('tacit.')) return 'tacit';
		if (namespace.startsWith('daily.')) return 'daily';
		if (namespace.startsWith('entity.')) return 'entity';
		return 'other';
	}

	function getLayerColor(layer: string): string {
		switch (layer) {
			case 'tacit': return 'badge-primary';
			case 'daily': return 'badge-secondary';
			case 'entity': return 'badge-accent';
			default: return 'badge-ghost';
		}
	}

	function formatDate(dateStr: string): string {
		if (!dateStr) return 'Never';
		return new Date(dateStr).toLocaleString();
	}

	function handleSearchKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			searchMemoriesHandler();
		}
	}
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Memories</h2>
		<p class="text-sm text-base-content/60">Browse and manage what the agent remembers about you</p>
	</div>
	<Button type="ghost" onclick={() => { loadMemories(); loadStats(); }}>
		<RefreshCw class="w-4 h-4 mr-2" />
		Refresh
	</Button>
</div>

<!-- Stats Cards -->
{#if stats}
<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
	<div class="bg-base-200 rounded-lg p-4">
		<div class="text-2xl font-bold text-base-content">{stats.totalCount}</div>
		<div class="text-sm text-base-content/60">Total Memories</div>
	</div>
	<div class="bg-primary/10 rounded-lg p-4">
		<div class="text-2xl font-bold text-primary">{stats.layerCounts?.tacit || 0}</div>
		<div class="text-sm text-base-content/60">Tacit (Core)</div>
	</div>
	<div class="bg-secondary/10 rounded-lg p-4">
		<div class="text-2xl font-bold text-secondary">{stats.layerCounts?.daily || 0}</div>
		<div class="text-sm text-base-content/60">Daily (Context)</div>
	</div>
	<div class="bg-accent/10 rounded-lg p-4">
		<div class="text-2xl font-bold text-accent">{stats.layerCounts?.entity || 0}</div>
		<div class="text-sm text-base-content/60">Entity (References)</div>
	</div>
</div>
{/if}

<div class="grid lg:grid-cols-12 gap-6">
	<!-- Namespace Tree -->
	<div class="lg:col-span-2">
		<Card>
			<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
				<FolderTree class="w-4 h-4" />
				Namespaces
			</h2>
			<div class="space-y-1">
				<button
					class="w-full text-left px-2 py-1 rounded text-sm transition-colors {selectedNamespace === '' ? 'bg-primary/20 text-primary' : 'hover:bg-base-200'}"
					onclick={() => selectNamespaceFilter('')}
				>
					All
				</button>
				{#if stats?.namespaces}
					{#each stats.namespaces as ns}
						<button
							class="w-full text-left px-2 py-1 rounded text-sm transition-colors truncate {selectedNamespace === ns ? 'bg-primary/20 text-primary' : 'hover:bg-base-200'}"
							onclick={() => selectNamespaceFilter(ns)}
							title={ns}
						>
							{ns}
						</button>
					{/each}
				{/if}
			</div>
		</Card>
	</div>

	<!-- Memory List -->
	<div class="lg:col-span-5">
		<Card>
			<!-- Search -->
			<div class="flex gap-2 mb-4">
				<div class="relative flex-1">
					<Search class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-base-content/40" />
					<input
						type="text"
						placeholder="Search memories..."
						class="input input-bordered w-full pl-10"
						bind:value={searchQuery}
						onkeydown={handleSearchKeydown}
					/>
				</div>
				<Button type="primary" onclick={searchMemoriesHandler}>Search</Button>
			</div>

			<!-- Memory List -->
			{#if isLoading}
				<div class="py-8 text-center text-base-content/60">Loading...</div>
			{:else if memories.length === 0}
				<div class="py-8 text-center text-base-content/60">
					<Brain class="w-8 h-8 mx-auto mb-2 opacity-50" />
					<p>No memories found</p>
				</div>
			{:else}
				<div class="space-y-2 max-h-[60vh] overflow-y-auto">
					{#each memories as memory}
						<div
							class="p-3 rounded-lg transition-colors cursor-pointer {selectedMemory?.id === memory.id ? 'bg-primary/10 border border-primary/30' : 'bg-base-200 hover:bg-base-300'}"
							onclick={() => selectMemory(memory)}
							onkeydown={(e) => e.key === 'Enter' && selectMemory(memory)}
							role="button"
							tabindex="0"
						>
							<div class="flex items-start justify-between gap-2 mb-1">
								<span class="font-medium text-sm truncate flex-1">{memory.key}</span>
								<span class="badge badge-sm {getLayerColor(getLayerFromNamespace(memory.namespace))}">
									{getLayerFromNamespace(memory.namespace)}
								</span>
							</div>
							<p class="text-sm text-base-content/70 line-clamp-2 mb-2">{memory.value}</p>
							<div class="flex items-center gap-3 text-xs text-base-content/50">
								<span class="truncate">{memory.namespace}</span>
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
					<div class="flex justify-center gap-2 mt-4 pt-4 border-t border-base-300">
						<Button
							type="ghost"
							size="sm"
							disabled={currentPage === 1}
							onclick={() => { currentPage--; loadMemories(); }}
						>
							Previous
						</Button>
						<span class="text-sm text-base-content/60 py-2">
							Page {currentPage} of {Math.ceil(total / pageSize)}
						</span>
						<Button
							type="ghost"
							size="sm"
							disabled={currentPage >= Math.ceil(total / pageSize)}
							onclick={() => { currentPage++; loadMemories(); }}
						>
							Next
						</Button>
					</div>
				{/if}
			{/if}
		</Card>
	</div>

	<!-- Memory Detail -->
	<div class="lg:col-span-5">
		<Card class="h-[calc(100vh-320px)]">
			{#if selectedMemory}
				<div class="flex items-center justify-between mb-4">
					<h2 class="font-display font-bold text-base-content truncate flex-1" title={selectedMemory.key}>
						{selectedMemory.key}
					</h2>
					<div class="flex gap-2">
						{#if isEditing}
							<Button type="ghost" size="sm" onclick={() => isEditing = false}>
								<X class="w-4 h-4" />
							</Button>
							<Button type="primary" size="sm" onclick={saveEdit}>
								<Save class="w-4 h-4 mr-1" />
								Save
							</Button>
						{:else}
							<Button type="ghost" size="sm" onclick={startEdit}>
								<Edit2 class="w-4 h-4" />
							</Button>
							<Button type="ghost" size="sm" onclick={() => deleteMemoryHandler(selectedMemory!)}>
								<Trash2 class="w-4 h-4 text-error" />
							</Button>
						{/if}
					</div>
				</div>

				<!-- Metadata -->
				<div class="mb-4 space-y-2 text-sm">
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Namespace:</span>
						<span class="badge badge-ghost">{selectedMemory.namespace}</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Layer:</span>
						<span class="badge {getLayerColor(getLayerFromNamespace(selectedMemory.namespace))}">
							{getLayerFromNamespace(selectedMemory.namespace)}
						</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Access Count:</span>
						<span>{selectedMemory.accessCount}</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Created:</span>
						<span class="text-base-content/70">{formatDate(selectedMemory.createdAt)}</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Updated:</span>
						<span class="text-base-content/70">{formatDate(selectedMemory.updatedAt)}</span>
					</div>
					<div class="flex items-center gap-2">
						<span class="text-base-content/60 w-24">Last Access:</span>
						<span class="text-base-content/70">{selectedMemory.accessedAt ? formatDate(selectedMemory.accessedAt) : 'Never'}</span>
					</div>
				</div>

				<!-- Tags -->
				{#if isEditing}
					<div class="mb-4">
						<label class="text-sm text-base-content/60 mb-1 block">Tags (comma-separated)</label>
						<input
							type="text"
							class="input input-bordered w-full"
							bind:value={editTags}
							placeholder="tag1, tag2, tag3"
						/>
					</div>
				{:else if selectedMemory.tags?.length}
					<div class="mb-4">
						<label class="text-sm text-base-content/60 mb-1 block">Tags</label>
						<div class="flex flex-wrap gap-1">
							{#each selectedMemory.tags as tag}
								<span class="badge badge-outline badge-sm">
									<Tag class="w-3 h-3 mr-1" />
									{tag}
								</span>
							{/each}
						</div>
					</div>
				{/if}

				<!-- Value -->
				<div class="flex-1">
					<label class="text-sm text-base-content/60 mb-1 block">Value</label>
					{#if isEditing}
						<textarea
							class="textarea textarea-bordered w-full h-48"
							bind:value={editValue}
						></textarea>
					{:else}
						<div class="bg-base-200 rounded-lg p-4 whitespace-pre-wrap text-sm max-h-48 overflow-y-auto">
							{selectedMemory.value}
						</div>
					{/if}
				</div>
			{:else}
				<div class="h-full flex items-center justify-center text-center">
					<div>
						<Brain class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
						<p class="text-base-content/60">Select a memory to view details</p>
					</div>
				</div>
			{/if}
		</Card>
	</div>
</div>
