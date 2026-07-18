<script lang="ts">
	import { t } from 'svelte-i18n';

	let { price, installed = false }: { price: string; installed?: boolean } = $props();
</script>

{#if installed}
	<span class="btn-market btn-market-installed">{$t('common.installed')}</span>
{:else if price === 'Get'}
	<span class="btn-market btn-market-get">{$t('common.get')}</span>
{:else}
	<span class="text-sm font-bold text-primary">{price}</span>
{/if}
