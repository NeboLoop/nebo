<script lang="ts">
	import { Copy, Check } from 'lucide-svelte';
	import { onMount } from 'svelte';

	let {
		code,
		language = 'plaintext',
		badgeText,
		badgeColor = 'bg-primary hover:bg-primary',
		title,
		footer,
		disabled = false,
		showLineNumbers = true,
		class: className,
		style,
		height
	}: {
		code: string;
		language?: string;
		badgeText?: string;
		badgeColor?: string;
		title?: string;
		footer?: any;
		disabled?: boolean;
		showLineNumbers?: boolean;
		class?: string;
		style?: string;
		height?: string | number;
	} = $props();

	let copied = $state(false);
	let highlightedHtml = $state('');

	onMount(async () => {
		await highlightCode();
	});

	async function highlightCode() {
		try {
			const { codeToHtml } = await import('shiki');
			highlightedHtml = await codeToHtml(code, {
				lang: language,
				theme: 'github-dark'
			});
		} catch (err) {
			// Fallback to plain code if highlighting fails
			console.warn('Syntax highlighting failed:', err);
			highlightedHtml = '';
		}
	}

	// Re-highlight when code changes
	$effect(() => {
		if (code) {
			highlightCode();
		}
	});

	const handleCopyCode = () => {
		navigator.clipboard.writeText(code);
		copied = true;
		setTimeout(() => (copied = false), 1500);
	};

	// Combine styles including height if provided
	let containerStyle = $derived(
		height
			? `${style || ''}; height: ${typeof height === 'number' ? `${height}px` : height}; display: flex; flex-direction: column;`
			: `${style || ''}; display: flex; flex-direction: column;`
	);

	// Split code into lines for line numbers
	let codeLines = $derived(code.split('\n'));
</script>

<div
	class={`rounded-lg overflow-hidden border border-border bg-base-200 ${className || ''}`}
	style={containerStyle}
>
	<div class="flex items-center px-4 py-2 justify-between border-b border-border/50 bg-base-300">
		<div class="flex items-center gap-2">
			{#if badgeText}
				<span class={`${badgeColor} text-primary-content text-xs px-1.5 py-0.5 rounded`}>
					{badgeText}
				</span>
			{/if}
			{#if title}
				<span class="text-xs font-medium text-base-content/60">{title}</span>
			{/if}
		</div>
		<button
			type="button"
			{disabled}
			onclick={handleCopyCode}
			class="flex items-center justify-center w-8 h-8 rounded hover:bg-base-content/10 text-base-content/60 hover:text-base-content transition-colors"
		>
			{#if copied}
				<Check class="h-4 w-4 text-success" />
			{:else}
				<Copy class="h-4 w-4" />
			{/if}
		</button>
	</div>
	<div class="flex-1 overflow-auto">
		{#if highlightedHtml && !showLineNumbers}
			<div class="shiki-container p-4">
				<!-- eslint-disable-next-line svelte/no-at-html-tags -->
				{@html highlightedHtml}
			</div>
		{:else}
			<div class="flex text-sm font-mono">
				{#if showLineNumbers}
					<div class="flex-shrink-0 py-4 pl-4 pr-2 text-right select-none text-base-content/40 border-r border-border/30">
						{#each codeLines as _, i}
							<div class="leading-6">{i + 1}</div>
						{/each}
					</div>
				{/if}
				<div class="flex-1 overflow-x-auto">
					{#if highlightedHtml}
						<div class="shiki-container p-4 [&_pre]:!bg-transparent [&_pre]:!p-0 [&_code]:leading-6">
							<!-- eslint-disable-next-line svelte/no-at-html-tags -->
							{@html highlightedHtml}
						</div>
					{:else}
						<pre class="p-4 text-base-content/70 leading-6"><code>{code}</code></pre>
					{/if}
				</div>
			</div>
		{/if}
	</div>
	{#if footer}
		<div class="px-4 py-2 border-t border-border/50 text-base-content/60 bg-base-300">
			{@render footer()}
		</div>
	{/if}
</div>
