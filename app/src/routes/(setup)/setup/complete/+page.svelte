<script lang="ts">
	import { goto } from '$app/navigation';
	import { completeSetup } from '$lib/api';
	import { setup } from '$lib/stores/setup.svelte';
	import { StepCard } from '$lib/components/setup';
	import { CheckCircle } from 'lucide-svelte';

	let loading = $state(true);
	let error = $state('');
	let completed = $state(false);

	// On mount, call the API to complete setup
	$effect(() => {
		if (!completed && loading) {
			completeSetupWizard();
		}
	});

	async function completeSetupWizard() {
		try {
			await completeSetup();
			setup.markComplete();
			completed = true;
		} catch (e: unknown) {
			error = e instanceof Error ? e.message : 'Failed to complete setup';
		} finally {
			loading = false;
		}
	}

	function handleOpenChat() {
		goto('/agent');
	}

	function handleGoToSettings() {
		goto('/settings');
	}

	// Check if advanced mode (has extra steps)
	let isAdvancedMode = $derived(setup.state.mode === 'advanced');
</script>

<svelte:head>
	<title>Setup Complete - Nebo Setup</title>
</svelte:head>

<StepCard
	title="Setup Complete!"
	description="Your Nebo is ready to use."
>
	{#if loading}
		<div class="flex flex-col items-center justify-center py-8">
			<span class="loading loading-spinner loading-lg text-primary"></span>
			<p class="mt-4 text-base-content/70">Completing setup...</p>
		</div>
	{:else if error}
		<div class="alert alert-error mb-4">
			<span>{error}</span>
		</div>
		<div class="flex justify-center">
			<button class="btn btn-primary" onclick={completeSetupWizard}>
				Try Again
			</button>
		</div>
	{:else}
		<!-- Success animation -->
		<div class="flex flex-col items-center py-6">
			<div class="animate-bounce-once">
				<CheckCircle class="w-24 h-24 text-success" />
			</div>
		</div>

		<!-- Configuration summary -->
		<div class="bg-base-200 rounded-lg p-6 mb-6">
			<h3 class="font-semibold mb-4">Configuration Summary</h3>
			<ul class="space-y-3">
				<li class="flex items-center gap-3">
					<CheckCircle class="w-5 h-5 text-success flex-shrink-0" />
					<span>Account created</span>
				</li>
				<li class="flex items-center gap-3">
					<CheckCircle class="w-5 h-5 text-success flex-shrink-0" />
					<span>AI Provider configured</span>
				</li>
				{#if isAdvancedMode}
					<li class="flex items-center gap-3">
						<CheckCircle class="w-5 h-5 text-success flex-shrink-0" />
						<span>Models configured</span>
					</li>
					<li class="flex items-center gap-3">
						<CheckCircle class="w-5 h-5 text-success flex-shrink-0" />
						<span>Permissions set</span>
					</li>
					<li class="flex items-center gap-3">
						<CheckCircle class="w-5 h-5 text-success flex-shrink-0" />
						<span>Personality customized</span>
					</li>
				{/if}
			</ul>
		</div>

		<!-- Action buttons -->
		<div class="flex flex-col sm:flex-row gap-4 justify-center">
			<button class="btn btn-primary" onclick={handleOpenChat}>
				Open Chat
			</button>
			<button class="btn btn-outline" onclick={handleGoToSettings}>
				Go to Settings
			</button>
		</div>
	{/if}
</StepCard>
