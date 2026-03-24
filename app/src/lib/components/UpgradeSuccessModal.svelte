<!--
  Upgrade Success Modal
  Shown when a plan_changed WebSocket event fires after a successful payment
-->

<script lang="ts">
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

<Modal bind:show title="Upgrade Complete" {onclose}>
	<div class="flex flex-col items-center text-center gap-4 py-4">
		<div class="w-16 h-16 rounded-full bg-success/15 flex items-center justify-center">
			<CheckCircle class="w-8 h-8 text-success" />
		</div>

		<div>
			<h3 class="text-lg font-bold text-base-content">You're all set!</h3>
			<p class="text-base-content/70 mt-1">Your <span class="font-semibold">{planDisplay}</span> plan is now active. Enjoy your new capabilities.</p>
		</div>
	</div>

	{#snippet footer()}
		<button class="btn btn-primary" onclick={handleClose}>
			Got it
		</button>
	{/snippet}
</Modal>
