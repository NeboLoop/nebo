<script lang="ts">
	import { onMount } from 'svelte';
	import { Heart, Save, RotateCcw, Clock, Info } from 'lucide-svelte';
	import * as api from '$lib/api/gobot';
	import Button from '$lib/components/ui/Button.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import MarkdownEditor from '$lib/components/ui/MarkdownEditor.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveSuccess = $state(false);
	let saveError = $state('');
	let content = $state('');
	let originalContent = $state('');
	let intervalMinutes = $state(30);
	let originalInterval = $state(30);

	const intervalOptions = [
		{ value: 5, label: '5 minutes' },
		{ value: 10, label: '10 minutes' },
		{ value: 15, label: '15 minutes' },
		{ value: 30, label: '30 minutes' },
		{ value: 60, label: '1 hour' },
		{ value: 120, label: '2 hours' },
		{ value: 240, label: '4 hours' },
		{ value: 480, label: '8 hours' },
		{ value: 1440, label: '24 hours' }
	];

	const defaultTemplate = `# Proactive Tasks

This file defines tasks that GoBot checks periodically.
Write tasks in plain language - the agent will interpret and act on them.

## Every Check-In
- Check my email inbox for urgent items requiring response
- Review calendar for upcoming meetings needing prep
- Monitor project deadlines and flag any at risk

## Daily (check once per day)
- Summarize yesterday's accomplishments
- Generate today's priority list
- Check business metrics

## Weekly (Monday mornings)
- Draft weekly status update
- Review and clean up stale tasks
- Analyze week-over-week trends

---

**Tips:**
- Be specific about what "urgent" or "important" means to you
- Include context so the agent knows how to help
- The agent will respond with "HEARTBEAT_OK" if nothing needs attention
`;

	onMount(async () => {
		try {
			const [heartbeatRes, settingsRes] = await Promise.all([
				api.getHeartbeat(),
				api.getAgentSettings()
			]);
			content = heartbeatRes.content || '';
			if (!content.trim()) {
				content = defaultTemplate;
			}
			originalContent = content;
			intervalMinutes = settingsRes.settings.heartbeatIntervalMinutes || 30;
			originalInterval = intervalMinutes;
		} catch (err) {
			console.error('Failed to load heartbeat:', err);
			content = defaultTemplate;
			originalContent = content;
		} finally {
			isLoading = false;
		}
	});

	async function handleSave() {
		isSaving = true;
		saveSuccess = false;
		saveError = '';
		try {
			// Save both heartbeat content and interval setting
			const settingsRes = await api.getAgentSettings();
			await Promise.all([
				api.updateHeartbeat({ content }),
				api.updateAgentSettings({
					...settingsRes.settings,
					heartbeatIntervalMinutes: intervalMinutes
				})
			]);
			saveSuccess = true;
			originalContent = content;
			originalInterval = intervalMinutes;
			setTimeout(() => (saveSuccess = false), 3000);
		} catch (err: any) {
			saveError = err?.message || 'Failed to save heartbeat settings';
		} finally {
			isSaving = false;
		}
	}

	function handleReset() {
		content = defaultTemplate;
		intervalMinutes = 30;
		saveSuccess = false;
		saveError = '';
	}

	const hasChanges = $derived(content !== originalContent || intervalMinutes !== originalInterval);
</script>

<div class="flex flex-col h-full min-h-0">
	<!-- Header -->
	<div class="shrink-0 mb-4">
		<div class="flex items-center gap-3 mb-4">
			<div class="w-10 h-10 rounded-xl bg-error/10 flex items-center justify-center">
				<Heart class="w-5 h-5 text-error" />
			</div>
			<div>
				<h2 class="text-lg font-semibold text-base-content">Heartbeat Tasks</h2>
				<p class="text-sm text-base-content/60">Proactive tasks the agent checks periodically</p>
			</div>
		</div>

		<div class="bg-base-200 rounded-lg p-4">
			<div class="flex items-start gap-3">
				<Clock class="w-5 h-5 text-primary mt-0.5" />
				<div class="flex-1">
					<div class="flex items-center justify-between gap-4">
						<div class="text-sm">
							<p class="font-medium text-base-content">Check Interval</p>
							<p class="text-base-content/60">
								How often the agent reviews tasks and takes action
							</p>
						</div>
						<select
							bind:value={intervalMinutes}
							class="select select-bordered select-sm w-36"
							disabled={isLoading}
						>
							{#each intervalOptions as opt}
								<option value={opt.value}>{opt.label}</option>
							{/each}
						</select>
					</div>
				</div>
			</div>
		</div>
	</div>

	<!-- Editor - fills remaining space -->
	{#if isLoading}
		<div class="flex-1 flex flex-col items-center justify-center gap-4">
			<Spinner size={32} />
			<p class="text-sm text-base-content/60">Loading heartbeat tasks...</p>
		</div>
	{:else}
		<MarkdownEditor
			bind:value={content}
			placeholder="Enter your proactive tasks in Markdown..."
			class="flex-1"
		/>

		<div class="shrink-0 flex items-center gap-2 text-xs text-base-content/50 mt-2">
			<Info class="w-3.5 h-3.5" />
			<span>Saved to: ~/.gobot/HEARTBEAT.md</span>
		</div>
	{/if}

	<!-- Alerts -->
	{#if saveSuccess}
		<div class="shrink-0 mt-4">
			<Alert type="success" title="Saved">Heartbeat settings have been updated.</Alert>
		</div>
	{/if}

	{#if saveError}
		<div class="shrink-0 mt-4">
			<Alert type="error" title="Error">{saveError}</Alert>
		</div>
	{/if}

	<!-- Footer buttons -->
	<div class="shrink-0 flex justify-between mt-4">
		<Button type="ghost" onclick={handleReset} disabled={isLoading}>
			<RotateCcw class="w-4 h-4 mr-2" />
			Reset to Template
		</Button>
		<Button type="primary" onclick={handleSave} disabled={isSaving || !hasChanges || isLoading}>
			{#if isSaving}
				<Spinner size={16} />
				<span class="ml-2">Saving...</span>
			{:else}
				<Save class="w-4 h-4 mr-2" />
				Save Settings
			{/if}
		</Button>
	</div>
</div>
