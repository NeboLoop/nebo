<script lang="ts">
	import { Copy, Check } from 'lucide-svelte';

	let { code, compact = false, inline = false }: { code: string; compact?: boolean; inline?: boolean } = $props();
	let copied = $state(false);

	function handleClick(e: MouseEvent) {
		e.stopPropagation();
		e.preventDefault();
		navigator.clipboard.writeText(code);
		copied = true;
		setTimeout(() => copied = false, 2000);
	}
</script>

{#if inline}
	<button type="button" onclick={handleClick} class="flex items-center gap-1.5 mt-1 group/code">
		<span class="font-mono text-sm text-base-content/40 group-hover/code:text-primary transition-colors">{code}</span>
		{#if copied}
			<Check class="w-3 h-3 text-success shrink-0" />
		{:else}
			<Copy class="w-3 h-3 text-base-content/40 group-hover/code:text-base-content/60 shrink-0 transition-colors" />
		{/if}
	</button>
{:else}
	<div class="flex items-center gap-3 mt-3">
		<button type="button" onclick={handleClick} class="flex items-center gap-2 px-3 py-1.5 rounded-full bg-base-content/10 hover:bg-base-content/15 transition-colors">
			<span class="font-mono text-sm font-bold tracking-wider text-primary">{code}</span>
			{#if copied}
				<Check class="w-3.5 h-3.5 text-success shrink-0" />
			{:else}
				<Copy class="w-3.5 h-3.5 text-base-content/40 shrink-0" />
			{/if}
		</button>
	</div>
{/if}
