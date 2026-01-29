<!--
  Step Navigation Component
  Navigation buttons for setup wizard steps
  Uses DaisyUI btn classes
-->

<script lang="ts">
	import { Loader2 } from 'lucide-svelte';

	let {
		showBack = false,
		showSkip = false,
		nextLabel = 'Continue',
		loading = false,
		onback,
		onskip,
		onnext,
		class: extraClass = ''
	}: {
		showBack?: boolean;
		showSkip?: boolean;
		nextLabel?: string;
		loading?: boolean;
		onback?: () => void;
		onskip?: () => void;
		onnext?: () => void;
		class?: string;
	} = $props();
</script>

<div class="flex justify-between items-center gap-4 {extraClass}">
	<div>
		{#if showBack}
			<button type="button" class="btn btn-ghost" onclick={onback} disabled={loading}>
				Back
			</button>
		{/if}
	</div>

	<div class="flex gap-2">
		{#if showSkip}
			<button type="button" class="btn btn-ghost" onclick={onskip} disabled={loading}>
				Skip
			</button>
		{/if}
		<button type="button" class="btn btn-primary" onclick={onnext} disabled={loading}>
			{#if loading}
				<Loader2 class="h-4 w-4 animate-spin" />
			{/if}
			{nextLabel}
		</button>
	</div>
</div>
