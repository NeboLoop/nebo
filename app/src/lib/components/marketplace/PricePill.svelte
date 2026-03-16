<script lang="ts">
	let { price, installed = false }: { price: string; installed?: boolean } = $props();
</script>

{#if installed}
	<span class="btn-market btn-market-installed">Installed</span>
{:else if price === 'Get'}
	<span class="btn-market btn-market-get">{price}</span>
{:else}
	<span class="text-sm font-bold text-primary">{price}</span>
{/if}
