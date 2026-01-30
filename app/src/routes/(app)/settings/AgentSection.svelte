<script lang="ts">
	import { onMount } from 'svelte';
	import { Bot, Shield, AlertTriangle, Zap } from 'lucide-svelte';
	import * as api from '$lib/api/gobot';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveSuccess = $state(false);
	let saveError = $state('');

	// Agent settings
	let autonomousMode = $state(false);
	let autoApproveRead = $state(true);
	let autoApproveWrite = $state(false);
	let autoApproveBash = $state(false);
	let heartbeatIntervalMinutes = $state(30);

	// Original values for change detection
	let originalSettings = $state({
		autonomousMode: false,
		autoApproveRead: true,
		autoApproveWrite: false,
		autoApproveBash: false,
		heartbeatIntervalMinutes: 30
	});

	// Load settings on mount
	onMount(async () => {
		try {
			const response = await api.getAgentSettings();
			const settings = response.settings;
			autonomousMode = settings.autonomousMode ?? false;
			autoApproveRead = settings.autoApproveRead ?? true;
			autoApproveWrite = settings.autoApproveWrite ?? false;
			autoApproveBash = settings.autoApproveBash ?? false;
			heartbeatIntervalMinutes = settings.heartbeatIntervalMinutes ?? 30;

			originalSettings = {
				autonomousMode,
				autoApproveRead,
				autoApproveWrite,
				autoApproveBash,
				heartbeatIntervalMinutes
			};
		} catch (err) {
			console.error('Failed to load agent settings:', err);
			// Use defaults if no settings exist yet
		} finally {
			isLoading = false;
		}
	});

	async function handleSave() {
		isSaving = true;
		saveSuccess = false;
		saveError = '';

		try {
			await api.updateAgentSettings({
				autonomousMode,
				autoApproveRead,
				autoApproveWrite,
				autoApproveBash,
				heartbeatIntervalMinutes
			});
			saveSuccess = true;
			originalSettings = {
				autonomousMode,
				autoApproveRead,
				autoApproveWrite,
				autoApproveBash,
				heartbeatIntervalMinutes
			};
		} catch (err: any) {
			saveError = err?.message || 'Failed to save settings';
		} finally {
			isSaving = false;
		}
	}

	function clearMessages() {
		saveSuccess = false;
		saveError = '';
	}

	// When autonomous mode is enabled, enable all auto-approvals
	function handleAutonomousModeChange() {
		if (autonomousMode) {
			autoApproveRead = true;
			autoApproveWrite = true;
			autoApproveBash = true;
		}
		clearMessages();
	}

	// Track if there are unsaved changes
	const hasChanges = $derived(
		autonomousMode !== originalSettings.autonomousMode ||
		autoApproveRead !== originalSettings.autoApproveRead ||
		autoApproveWrite !== originalSettings.autoApproveWrite ||
		autoApproveBash !== originalSettings.autoApproveBash
	);
</script>

<div class="space-y-6">
	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading agent settings...</p>
			</div>
		</Card>
	{:else}
		<!-- Autonomous Mode (Dangerous) -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div class="w-10 h-10 rounded-xl bg-error/10 flex items-center justify-center">
					<Zap class="w-5 h-5 text-error" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Autonomous Mode</h2>
					<p class="text-sm text-base-content/60">Run the agent without approval prompts</p>
				</div>
			</div>

			<div class="space-y-4">
				<div class="flex items-start justify-between py-3 border-b border-base-content/20">
					<div class="flex-1 pr-4">
						<p class="text-sm font-medium text-base-content flex items-center gap-2">
							<AlertTriangle class="w-4 h-4 text-warning" />
							100% Autonomous Mode
						</p>
						<p class="text-xs text-base-content/60 mt-1">
							The agent will execute ALL tools without asking for permission.
							This includes shell commands, file modifications, and network requests.
							<strong class="text-error">Use with extreme caution.</strong>
						</p>
					</div>
					<Toggle
						bind:checked={autonomousMode}
						onchange={handleAutonomousModeChange}
					/>
				</div>

				{#if autonomousMode}
					<Alert type="warning" title="Autonomous Mode Enabled">
						The agent will bypass all approval prompts and execute tools automatically.
						Make sure you trust the prompts you're sending and have backups of important data.
					</Alert>
				{/if}
			</div>
		</Card>

		<!-- Tool Approvals (when not in autonomous mode) -->
		{#if !autonomousMode}
			<Card>
				<div class="flex items-center gap-3 mb-6">
					<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
						<Shield class="w-5 h-5 text-primary" />
					</div>
					<div>
						<h2 class="text-lg font-semibold text-base-content">Tool Permissions</h2>
						<p class="text-sm text-base-content/60">Configure which tools auto-approve</p>
					</div>
				</div>

				<div class="space-y-4">
					<div class="flex items-center justify-between py-3 border-b border-base-content/20">
						<div>
							<p class="text-sm font-medium text-base-content">Auto-approve File Reads</p>
							<p class="text-xs text-base-content/60">Allow reading files without prompting</p>
						</div>
						<Toggle bind:checked={autoApproveRead} onchange={clearMessages} />
					</div>

					<div class="flex items-center justify-between py-3 border-b border-base-content/20">
						<div>
							<p class="text-sm font-medium text-base-content">Auto-approve File Writes</p>
							<p class="text-xs text-base-content/60">Allow creating/editing files without prompting</p>
						</div>
						<Toggle bind:checked={autoApproveWrite} onchange={clearMessages} />
					</div>

					<div class="flex items-center justify-between py-3">
						<div>
							<p class="text-sm font-medium text-base-content">Auto-approve Shell Commands</p>
							<p class="text-xs text-base-content/60">Allow executing bash commands without prompting</p>
						</div>
						<Toggle bind:checked={autoApproveBash} onchange={clearMessages} />
					</div>
				</div>
			</Card>
		{/if}

		<!-- Agent Status Info -->
		<Card>
			<div class="flex items-center gap-3 mb-4">
				<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
					<Bot class="w-5 h-5 text-secondary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Starting the Agent</h2>
					<p class="text-sm text-base-content/60">Run the agent to process tasks</p>
				</div>
			</div>

			<div class="bg-base-200 rounded-lg p-4">
				<p class="text-sm text-base-content/70 mb-2">Run this command in your terminal:</p>
				<code class="block bg-base-300 rounded px-3 py-2 text-sm font-mono text-base-content">
					gobot agent
				</code>
				<p class="text-xs text-base-content/50 mt-2">
					The agent will automatically use the settings configured above.
				</p>
			</div>
		</Card>

		<!-- Save Button -->
		{#if saveSuccess}
			<Alert type="success" title="Saved">Agent settings have been updated.</Alert>
		{/if}

		{#if saveError}
			<Alert type="error" title="Error">{saveError}</Alert>
		{/if}

		<div class="flex justify-end">
			<Button type="primary" onclick={handleSave} disabled={isSaving || !hasChanges}>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Settings
				{/if}
			</Button>
		</div>
	{/if}
</div>
