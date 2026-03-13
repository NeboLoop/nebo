<script lang="ts">
	import type { Snippet } from 'svelte';
	import '$src/app.css';

	let { children }: { children: Snippet } = $props();
</script>

<svelte:head>
	<meta name="viewport" content="width=device-width, initial-scale=1" />
</svelte:head>

{@render children()}
