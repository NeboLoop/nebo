<script lang="ts">
	import { goto } from '$app/navigation';
	import { setupStatus } from '$lib/api';
	import { setup } from '$lib/stores/setup.svelte';
	import SecurityWarning from '$lib/components/setup/SecurityWarning.svelte';
	import StepCard from '$lib/components/setup/StepCard.svelte';
	import { Sparkles, Zap, Settings } from 'lucide-svelte';

	let checkingStatus = $state(true);
	let securityAcknowledged = $state(false);
	let selectedMode = $state<'quickstart' | 'advanced'>('quickstart');

	let canContinue = $derived(securityAcknowledged);

	$effect(() => {
		checkSetupStatus();
	});

	async function checkSetupStatus() {
		try {
			const status = await setupStatus();
			if (!status.setupRequired) {
				goto('/app');
			}
		} catch (e) {
			console.error('Failed to check setup status', e);
		} finally {
			checkingStatus = false;
		}
	}

	function handleContinue() {
		setup.setMode(selectedMode);
		setup.acknowledgeSecruity();
		goto('/setup/account');
	}
</script>

<svelte:head>
	<title>Welcome - GoBot Setup</title>
	<meta name="description" content="Set up your GoBot instance and configure your personal AI agent." />
</svelte:head>

{#if checkingStatus}
	<div class="flex items-center justify-center p-8">
		<span class="loading loading-spinner loading-lg"></span>
	</div>
{:else}
	<StepCard
		title="Welcome to GoBot"
		description="Your personal AI agent that runs locally on your machine."
	>
		{#snippet children()}
			<div class="space-y-6">
				<!-- GoBot Capabilities -->
				<div class="prose prose-sm max-w-none">
					<p>
						GoBot is a powerful AI assistant that can help you with a wide range of tasks:
					</p>
					<ul class="list-disc list-inside space-y-1 mt-2 text-base-content/80">
						<li>Execute shell commands and scripts</li>
						<li>Read, write, and manage files</li>
						<li>Browse the web and gather information</li>
						<li>Remember conversations and learn your preferences</li>
						<li>Automate repetitive tasks with skills and plugins</li>
						<li>Connect via Web, CLI, Telegram, Discord, and more</li>
					</ul>
				</div>

				<!-- Security Warning -->
				<SecurityWarning />

				<!-- Security Acknowledgment -->
				<div class="form-control">
					<label class="label cursor-pointer justify-start gap-3">
						<input
							type="checkbox"
							bind:checked={securityAcknowledged}
							class="checkbox checkbox-primary"
						/>
						<span class="label-text">
							I understand GoBot's capabilities and will configure appropriate permissions
						</span>
					</label>
				</div>

				<!-- Mode Selection -->
				<div class="divider">Setup Mode</div>

				<div class="grid gap-4 md:grid-cols-2">
					<!-- Quick Start -->
					<label class="cursor-pointer">
						<input
							type="radio"
							name="setupMode"
							value="quickstart"
							bind:group={selectedMode}
							class="hidden peer"
						/>
						<div class="card bg-base-200 border-2 border-transparent peer-checked:border-primary peer-checked:bg-primary/10 transition-all">
							<div class="card-body p-4">
								<div class="flex items-center gap-3">
									<div class="p-2 rounded-lg bg-primary/20">
										<Zap class="h-6 w-6 text-primary" />
									</div>
									<div>
										<h3 class="font-bold">Quick Start</h3>
										<p class="text-sm text-base-content/70">Recommended</p>
									</div>
								</div>
								<p class="text-sm mt-2 text-base-content/80">
									Get up and running fast with sensible defaults. You can always customize later.
								</p>
								<div class="text-xs text-base-content/60 mt-2">
									3 steps: Account, Provider, Done
								</div>
							</div>
						</div>
					</label>

					<!-- Advanced Setup -->
					<label class="cursor-pointer">
						<input
							type="radio"
							name="setupMode"
							value="advanced"
							bind:group={selectedMode}
							class="hidden peer"
						/>
						<div class="card bg-base-200 border-2 border-transparent peer-checked:border-primary peer-checked:bg-primary/10 transition-all">
							<div class="card-body p-4">
								<div class="flex items-center gap-3">
									<div class="p-2 rounded-lg bg-secondary/20">
										<Settings class="h-6 w-6 text-secondary" />
									</div>
									<div>
										<h3 class="font-bold">Advanced Setup</h3>
										<p class="text-sm text-base-content/70">Full control</p>
									</div>
								</div>
								<p class="text-sm mt-2 text-base-content/80">
									Configure models, permissions, and personality during setup.
								</p>
								<div class="text-xs text-base-content/60 mt-2">
									6 steps: Account, Provider, Models, Permissions, Personality, Done
								</div>
							</div>
						</div>
					</label>
				</div>

				<!-- Continue Button -->
				<div class="flex justify-end pt-4">
					<button
						type="button"
						class="btn btn-primary"
						disabled={!canContinue}
						onclick={handleContinue}
					>
						<Sparkles class="h-4 w-4" />
						Get Started
					</button>
				</div>
			</div>
		{/snippet}
	</StepCard>
{/if}
