<script lang="ts">
	import { marked } from 'marked';
	import { Eye, Code, Columns } from 'lucide-svelte';

	interface Props {
		value: string;
		placeholder?: string;
		class?: string;
		onchange?: (value: string) => void;
	}

	let {
		value = $bindable(''),
		placeholder = 'Write markdown here...',
		class: className = '',
		onchange
	}: Props = $props();

	type ViewMode = 'edit' | 'preview' | 'split';
	let viewMode = $state<ViewMode>('preview');

	// Configure marked
	marked.setOptions({
		breaks: true,
		gfm: true
	});

	let html = $derived(marked.parse(value || '') as string);

	function handleInput(e: Event) {
		const target = e.target as HTMLTextAreaElement;
		value = target.value;
		onchange?.(value);
	}
</script>

<div class="flex flex-col border border-base-content/20 rounded-lg overflow-hidden min-h-0 {className}">
	<!-- Toolbar -->
	<div class="flex items-center justify-between px-3 py-2 bg-base-200 border-b border-base-content/20 shrink-0">
		<span class="text-xs font-medium text-base-content/60">Markdown</span>
		<div class="flex gap-1">
			<button
				type="button"
				class="btn btn-xs btn-ghost {viewMode === 'edit' ? 'btn-active' : ''}"
				onclick={() => (viewMode = 'edit')}
				title="Edit only"
			>
				<Code class="w-3.5 h-3.5" />
			</button>
			<button
				type="button"
				class="btn btn-xs btn-ghost {viewMode === 'split' ? 'btn-active' : ''}"
				onclick={() => (viewMode = 'split')}
				title="Split view"
			>
				<Columns class="w-3.5 h-3.5" />
			</button>
			<button
				type="button"
				class="btn btn-xs btn-ghost {viewMode === 'preview' ? 'btn-active' : ''}"
				onclick={() => (viewMode = 'preview')}
				title="Preview only"
			>
				<Eye class="w-3.5 h-3.5" />
			</button>
		</div>
	</div>

	<!-- Editor Area -->
	<div class="flex flex-1 min-h-0">
		{#if viewMode === 'edit' || viewMode === 'split'}
			<div class="flex-1 min-h-0 {viewMode === 'split' ? 'border-r border-base-content/20' : ''}">
				<textarea
					{value}
					oninput={handleInput}
					class="w-full h-full p-4 bg-base-100 font-mono text-sm leading-relaxed resize-none focus:outline-none"
					{placeholder}
					spellcheck="false"
				></textarea>
			</div>
		{/if}

		{#if viewMode === 'preview' || viewMode === 'split'}
			<div class="flex-1 overflow-auto p-4 bg-base-100 min-h-0">
				{#if value.trim()}
					<div class="prose prose-sm max-w-none dark:prose-invert">
						{@html html}
					</div>
				{:else}
					<p class="text-sm text-base-content/40 italic">Preview will appear here...</p>
				{/if}
			</div>
		{/if}
	</div>
</div>
