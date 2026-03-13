<script lang="ts">
	import { page } from '$app/stores';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();

	const pageTitle = $derived(() => {
		const path = $page.url.pathname;
		if (path.startsWith('/store/roles')) return 'Roles';
		if (path.startsWith('/store/workflows')) return 'Workflows';
		if (path.startsWith('/store/skills')) return 'Skills';
		if (path.startsWith('/store/categories')) return 'Categories';
		return 'Marketplace';
	});
</script>

<svelte:head>
	<title>{pageTitle()} - Nebo</title>
</svelte:head>

{@render children()}
