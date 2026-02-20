<script lang="ts">
	import { parseMarkdown } from '$lib/utils/markdown-parser';

	interface Props {
		content: string;
		class?: string;
	}

	let { content, class: className = '' }: Props = $props();

	const MAX_RENDER_LENGTH = 50_000;
	let truncated = $derived(content?.length > MAX_RENDER_LENGTH);
	let showFull = $state(false);
	let displayContent = $derived(
		truncated && !showFull ? content.slice(0, MAX_RENDER_LENGTH) + '\n\n---\n*Content truncated*' : content
	);
	let html = $derived(parseMarkdown(displayContent));
	let container: HTMLDivElement;

	// Load X/Twitter widget script once
	function ensureTwitterWidget(): Promise<void> {
		return new Promise((resolve) => {
			if (typeof window === 'undefined') return resolve();

			const twttr = (window as any).twttr;
			if (twttr?.widgets) {
				resolve();
				return;
			}

			// Check if script is already loading
			if (document.querySelector('script[src*="platform.twitter.com/widgets.js"]')) {
				// Wait for it to load
				const check = setInterval(() => {
					if ((window as any).twttr?.widgets) {
						clearInterval(check);
						resolve();
					}
				}, 100);
				return;
			}

			const script = document.createElement('script');
			script.src = 'https://platform.twitter.com/widgets.js';
			script.async = true;
			script.charset = 'utf-8';
			script.onload = () => {
				const check = setInterval(() => {
					if ((window as any).twttr?.widgets) {
						clearInterval(check);
						resolve();
					}
				}, 50);
			};
			document.head.appendChild(script);
		});
	}

	// Hydrate tweet embeds whenever html changes
	$effect(() => {
		if (html.includes('twitter-tweet') && container) {
			// Tick to let Svelte update the DOM first
			setTimeout(async () => {
				await ensureTwitterWidget();
				(window as any).twttr.widgets.load(container);
			}, 0);
		}
	});
</script>

<div bind:this={container} class="prose prose-sm max-w-none dark:prose-invert {className}">
	{@html html}
	{#if truncated && !showFull}
		<button type="button" class="btn btn-xs btn-ghost mt-2" onclick={() => showFull = true}>
			Show full content ({Math.round(content.length / 1024)}KB)
		</button>
	{/if}
</div>
