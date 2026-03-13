<script lang="ts">
	import { onMount } from 'svelte';
	import { Clock, Info, RotateCcw, Save, Loader2 } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import MarkdownEditor from '$lib/components/ui/MarkdownEditor.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveSuccess = $state(false);
	let saveError = $state('');
	let content = $state('');
	let originalContent = $state('');
	let intervalMinutes = $state(30);

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

This file defines tasks that Nebo checks periodically.
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

	let currentSettings: Record<string, any> = {};

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
			currentSettings = settingsRes.settings;
			intervalMinutes = settingsRes.settings.heartbeatIntervalMinutes || 30;
		} catch (err) {
			console.error('Failed to load heartbeat:', err);
			content = defaultTemplate;
			originalContent = content;
		} finally {
			isLoading = false;
		}
	});

	async function handleIntervalChange() {
		try {
			await api.updateAgentSettings({
				...currentSettings,
				heartbeatIntervalMinutes: intervalMinutes
			});
			currentSettings = { ...currentSettings, heartbeatIntervalMinutes: intervalMinutes };
		} catch (err: any) {
			console.error('Failed to save interval:', err);
			saveError = err?.message || 'Failed to save interval';
		}
	}

	async function handleSave() {
		isSaving = true;
		saveSuccess = false;
		saveError = '';
		try {
			await api.updateHeartbeat({ content });
			saveSuccess = true;
			originalContent = content;
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
		handleIntervalChange();
		saveSuccess = false;
		saveError = '';
	}

	const hasChanges = $derived(content !== originalContent);
</script>

<div class="flex flex-col h-full min-h-0">
	<!-- Header -->
	<div class="shrink-0 mb-4">
		<div class="mb-4">
			<h2 class="font-display text-xl font-bold text-base-content mb-1">Heartbeat</h2>
			<p class="text-sm text-base-content/70">Proactive tasks the agent checks periodically</p>
		</div>

		<!-- Check Interval -->
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			<div class="flex items-center justify-between gap-4">
				<div class="flex items-center gap-3">
					<div class="w-9 h-9 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
						<Clock class="w-4.5 h-4.5 text-primary" />
					</div>
					<div>
						<p class="text-sm font-medium text-base-content">Check Interval</p>
						<p class="text-sm text-base-content/70">How often the agent reviews tasks and takes action</p>
					</div>
				</div>
				<select
					bind:value={intervalMinutes}
					onchange={handleIntervalChange}
					class="h-9 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
					disabled={isLoading}
				>
					{#each intervalOptions as opt}
						<option value={opt.value}>{opt.label}</option>
					{/each}
				</select>
			</div>
		</div>
	</div>

	<!-- Editor - fills remaining space -->
	{#if isLoading}
		<div class="flex-1 flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-sm text-base-content/70">Loading heartbeat tasks...</span>
		</div>
	{:else}
		<MarkdownEditor
			bind:value={content}
			placeholder="Enter your proactive tasks in Markdown..."
			class="flex-1"
		/>

		<div class="shrink-0 flex items-center gap-2 text-sm text-base-content/50 mt-2">
			<Info class="w-3.5 h-3.5" />
			<span>Saved to: HEARTBEAT.md in your Nebo data directory</span>
		</div>
	{/if}

	<!-- Feedback -->
	{#if saveSuccess}
		<div class="shrink-0 mt-3 rounded-xl bg-success/10 border border-success/20 px-4 py-3 text-sm text-success">
			Heartbeat settings have been updated.
		</div>
	{/if}

	{#if saveError}
		<div class="shrink-0 mt-3 rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-sm text-error">
			{saveError}
		</div>
	{/if}

	<!-- Footer buttons -->
	<div class="shrink-0 flex justify-between mt-4">
		<button
			type="button"
			class="h-9 px-4 rounded-xl bg-base-content/5 border border-base-content/10 text-sm font-medium text-base-content/70 hover:border-base-content/20 hover:text-base-content transition-colors flex items-center gap-2 disabled:opacity-30"
			onclick={handleReset}
			disabled={isLoading}
		>
			<RotateCcw class="w-4 h-4" />
			Reset to Template
		</button>
		<button
			type="button"
			class="h-9 px-5 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all flex items-center gap-2 disabled:opacity-30"
			onclick={handleSave}
			disabled={isSaving || !hasChanges || isLoading}
		>
			{#if isSaving}
				<Loader2 class="w-4 h-4 animate-spin" />
				Saving...
			{:else}
				<Save class="w-4 h-4" />
				Save Settings
			{/if}
		</button>
	</div>
</div>
