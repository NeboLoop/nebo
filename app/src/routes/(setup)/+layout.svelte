<script lang="ts">
	import { setup } from '$lib/stores/setup.svelte';
	import { WizardProgress } from '$lib/components/setup';

	let { children } = $props();

	// Step display names based on mode
	const quickstartSteps = ['Welcome', 'Account', 'Provider', 'Complete'];
	const advancedSteps = ['Welcome', 'Account', 'Provider', 'Models', 'Permissions', 'Personality', 'Complete'];

	// Derived step names based on current mode
	let steps = $derived(setup.state.mode === 'quickstart' ? quickstartSteps : advancedSteps);
</script>

<div class="layout-auth">
	<div class="relative z-10 w-full max-w-2xl">
		<a href="/" class="block text-center mb-6">
			<span class="font-display text-3xl font-black text-gradient"> GoBot </span>
		</a>
		<WizardProgress
			currentStep={setup.state.currentStep}
			totalSteps={setup.totalSteps}
			{steps}
			class="mb-8"
		/>
		<main id="main-content">
			{@render children()}
		</main>
		<p class="text-center mt-8 text-sm text-base-content/60">
			<a href="/" class="hover:text-base-content transition-colors"> Back to home </a>
		</p>
	</div>
</div>
