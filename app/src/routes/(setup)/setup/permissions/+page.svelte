<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { getAgentSettings, updateAgentSettings } from '$lib/api';
	import { setup } from '$lib/stores/setup.svelte';
	import { StepCard, StepNavigation } from '$lib/components/setup';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import { AlertTriangle, Shield, Zap } from 'lucide-svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let error = $state('');

	// Agent settings
	let autonomousMode = $state(false);
	let autoApproveRead = $state(true);
	let autoApproveWrite = $state(false);
	let autoApproveBash = $state(false);

	// Load settings on mount
	onMount(async () => {
		try {
			const response = await getAgentSettings();
			const settings = response.settings;
			autonomousMode = settings.autonomousMode ?? false;
			autoApproveRead = settings.autoApproveRead ?? true;
			autoApproveWrite = settings.autoApproveWrite ?? false;
			autoApproveBash = settings.autoApproveBash ?? false;
		} catch (err) {
			console.error('Failed to load agent settings:', err);
			// Use defaults if no settings exist yet
		} finally {
			isLoading = false;
		}
	});

	// When autonomous mode is enabled, enable all auto-approvals
	function handleAutonomousModeChange() {
		if (autonomousMode) {
			autoApproveRead = true;
			autoApproveWrite = true;
			autoApproveBash = true;
		}
	}

	async function handleSave() {
		isSaving = true;
		error = '';

		try {
			await updateAgentSettings({
				autonomousMode,
				autoApproveRead,
				autoApproveWrite,
				autoApproveBash
			});
			goto('/setup/personality');
		} catch (err: unknown) {
			error = err instanceof Error ? err.message : 'Failed to save settings';
		} finally {
			isSaving = false;
		}
	}

	function handleBack() {
		goto('/setup/models');
	}

	function handleSkip() {
		goto('/setup/personality');
	}
</script>

<svelte:head>
	<title>Permissions - GoBot Setup</title>
</svelte:head>

<StepCard
	title="Agent Permissions"
	description="Configure how much autonomy your agent has when executing actions."
>
	{#if isLoading}
		<div class="flex flex-col items-center justify-center gap-4 py-8">
			<span class="loading loading-spinner loading-lg"></span>
			<p class="text-sm text-base-content/60">Loading settings...</p>
		</div>
	{:else}
		{#if error}
			<Alert type="error" class="mb-4">{error}</Alert>
		{/if}

		<!-- Autonomous Mode Warning -->
		<div class="bg-error/10 border border-error/20 rounded-lg p-4 mb-6">
			<div class="flex items-start gap-3">
				<div class="w-10 h-10 rounded-xl bg-error/20 flex items-center justify-center shrink-0">
					<Zap class="w-5 h-5 text-error" />
				</div>
				<div class="flex-1">
					<div class="flex items-center justify-between">
						<div>
							<p class="font-semibold text-base-content flex items-center gap-2">
								<AlertTriangle class="w-4 h-4 text-warning" />
								Autonomous Mode
							</p>
							<p class="text-sm text-base-content/60 mt-1">
								Enable all auto-approvals. The agent will execute ALL tools without asking.
							</p>
						</div>
						<Toggle
							bind:checked={autonomousMode}
							onchange={handleAutonomousModeChange}
						/>
					</div>
					{#if autonomousMode}
						<Alert type="warning" class="mt-3">
							The agent will bypass all approval prompts. Use with caution.
						</Alert>
					{/if}
				</div>
			</div>
		</div>

		<!-- Tool Permissions -->
		<div class="bg-base-200 rounded-lg p-4 mb-6">
			<div class="flex items-center gap-3 mb-4">
				<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
					<Shield class="w-5 h-5 text-primary" />
				</div>
				<div>
					<h3 class="font-semibold text-base-content">Tool Permissions</h3>
					<p class="text-sm text-base-content/60">Configure which tools auto-approve</p>
				</div>
			</div>

			<div class="space-y-4">
				<div class="flex items-center justify-between py-3 border-b border-base-content/10">
					<div>
						<p class="text-sm font-medium text-base-content">Auto-approve File Reads</p>
						<p class="text-xs text-base-content/60">Allow reading files without prompting</p>
					</div>
					<Toggle
						bind:checked={autoApproveRead}
						disabled={autonomousMode}
					/>
				</div>

				<div class="flex items-center justify-between py-3 border-b border-base-content/10">
					<div>
						<p class="text-sm font-medium text-base-content">Auto-approve File Writes</p>
						<p class="text-xs text-base-content/60">Allow creating/editing files without prompting</p>
					</div>
					<Toggle
						bind:checked={autoApproveWrite}
						disabled={autonomousMode}
					/>
				</div>

				<div class="flex items-center justify-between py-3">
					<div>
						<p class="text-sm font-medium text-base-content">Auto-approve Shell Commands</p>
						<p class="text-xs text-base-content/60">Allow executing bash commands without prompting</p>
					</div>
					<Toggle
						bind:checked={autoApproveBash}
						disabled={autonomousMode}
					/>
				</div>
			</div>
		</div>

		<StepNavigation
			showBack={true}
			showSkip={true}
			onback={handleBack}
			onskip={handleSkip}
			onnext={handleSave}
			nextLabel="Save & Continue"
			loading={isSaving}
		/>
	{/if}
</StepCard>
