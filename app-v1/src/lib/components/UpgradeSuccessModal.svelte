<!--
  Upgrade Success Modal
  Shown when a plan_changed WebSocket event fires after a successful payment
-->

<script lang="ts">
	import { t } from 'svelte-i18n';
	import Modal from '$lib/components/ui/Modal.svelte';
	import { CheckCircle } from 'lucide-svelte';

	interface Props {
		show?: boolean;
		plan: string;
		onclose?: () => void;
	}

	let {
		show = $bindable(false),
		plan,
		onclose
	}: Props = $props();

	function handleClose() {
		show = false;
		onclose?.();
	}

	const planDisplay = $derived(plan ? plan.charAt(0).toUpperCase() + plan.slice(1) : 'new');
</script>

<Modal bind:show title={$t('upgradeSuccess.title')} {onclose}>
	<div class="flex flex-col items-center text-center gap-4 py-4">
		<div class="w-16 h-16 rounded-full bg-success/15 flex items-center justify-center">
			<CheckCircle class="w-8 h-8 text-success" />
		</div>

		<div>
			<h3 class="text-lg font-bold text-base-content">{$t('upgradeSuccess.heading')}</h3>
			<p class="text-base-content/70 mt-1">{$t('upgradeSuccess.description', { values: { plan: planDisplay } })}</p>
		</div>
	</div>

	{#snippet footer()}
		<button class="btn btn-primary" onclick={handleClose}>
			{$t('common.gotIt')}
		</button>
	{/snippet}
</Modal>
