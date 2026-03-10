<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import type { Snippet } from 'svelte';
	import { pageTitle, pageActions } from '$lib/stores/pageTitle';

	interface Props {
		title: string;
		actions?: Snippet;
		maxWidth?: 'sm' | 'md' | 'lg' | 'xl' | '2xl' | 'full';
		children: Snippet;
	}

	let { title, actions, maxWidth = 'xl', children }: Props = $props();

	const maxWidthClasses: Record<string, string> = {
		sm: 'max-w-[640px]',
		md: 'max-w-[768px]',
		lg: 'max-w-[1024px]',
		xl: 'max-w-[1280px]',
		'2xl': 'max-w-[1536px]',
		full: 'max-w-[1800px]'
	};

	onMount(() => {
		pageTitle.set(title);
		if (actions) {
			pageActions.set(actions);
		}
	});

	onDestroy(() => {
		pageTitle.set('');
		pageActions.set(null);
	});

	// Update title reactively if it changes
	$effect(() => {
		pageTitle.set(title);
	});

	// Update actions reactively if they change
	$effect(() => {
		pageActions.set(actions ?? null);
	});
</script>

<div class="mx-auto w-full {maxWidthClasses[maxWidth]} px-6 py-6 pb-8">
	{@render children()}
</div>
