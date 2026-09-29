<script lang="ts">
	import { t } from 'svelte-i18n';

	// `hire`: the card is an employee, which is hired, never got or installed.
	let { price, installed = false, hire = false }: { price: string; installed?: boolean; hire?: boolean } = $props();
</script>

{#if installed}
	<span class="btn-market btn-market-installed">{hire ? $t('installFlow.hired') : $t('common.installed')}</span>
{:else if price === 'Get'}
	<span class="btn-market btn-market-get">{hire ? $t('common.hire') : $t('common.get')}</span>
{:else}
	<span class="text-sm font-bold text-primary">{price}</span>
{/if}
