<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { Clock, Info, RotateCcw, Save, Loader2 } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import RichInput from '$lib/components/ui/RichInput.svelte';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveSuccess = $state(false);
	let saveError = $state('');
	let content = $state('');
	let originalContent = $state('');
	let intervalMinutes = $state(30);

	const intervalOptions = [
		{ value: 5, labelKey: 'settingsHeartbeat.intervals.5min' },
		{ value: 10, labelKey: 'settingsHeartbeat.intervals.10min' },
		{ value: 15, labelKey: 'settingsHeartbeat.intervals.15min' },
		{ value: 30, labelKey: 'settingsHeartbeat.intervals.30min' },
		{ value: 60, labelKey: 'settingsHeartbeat.intervals.1h' },
		{ value: 120, labelKey: 'settingsHeartbeat.intervals.2h' },
		{ value: 240, labelKey: 'settingsHeartbeat.intervals.4h' },
		{ value: 480, labelKey: 'settingsHeartbeat.intervals.8h' },
		{ value: 1440, labelKey: 'settingsHeartbeat.intervals.24h' }
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
			saveError = err?.message || $t('settingsHeartbeat.intervalSaveFailed');
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
			saveError = err?.message || $t('settingsHeartbeat.saveFailed');
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
			<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsHeartbeat.title')}</h2>
			<p class="text-base text-base-content/80">{$t('settingsHeartbeat.description')}</p>
		</div>

		<!-- Check Interval -->
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			<div class="flex items-center justify-between gap-4">
				<div class="flex items-center gap-3">
					<div class="w-9 h-9 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
						<Clock class="w-4.5 h-4.5 text-primary" />
					</div>
					<div>
						<p class="text-base font-medium text-base-content">{$t('settingsHeartbeat.checkInterval')}</p>
						<p class="text-base text-base-content/80">{$t('settingsHeartbeat.checkIntervalDesc')}</p>
					</div>
				</div>
				<select
					bind:value={intervalMinutes}
					onchange={handleIntervalChange}
					class="h-9 rounded-xl bg-base-content/5 border border-base-content/10 px-3 text-base focus:outline-none focus:border-primary/50 transition-colors"
					disabled={isLoading}
				>
					{#each intervalOptions as opt}
						<option value={opt.value}>{$t(opt.labelKey)}</option>
					{/each}
				</select>
			</div>
		</div>
	</div>

	<!-- Editor - fills remaining space -->
	{#if isLoading}
		<div class="flex-1 flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-base text-base-content/80">{$t('settingsHeartbeat.loadingTasks')}</span>
		</div>
	{:else}
		<RichInput
			bind:value={content}
			mode="full"
			placeholder={$t('settingsHeartbeat.placeholder')}
		/>

		<div class="shrink-0 flex items-center gap-2 text-base text-base-content/80 mt-2">
			<Info class="w-3.5 h-3.5" />
			<span>{$t('settingsHeartbeat.savedTo')}</span>
		</div>
	{/if}

	<!-- Feedback -->
	{#if saveSuccess}
		<div class="shrink-0 mt-3 rounded-xl bg-success/10 border border-success/20 px-4 py-3 text-base text-success">
			{$t('settingsHeartbeat.updated')}
		</div>
	{/if}

	{#if saveError}
		<div class="shrink-0 mt-3 rounded-xl bg-error/10 border border-error/20 px-4 py-3 text-base text-error">
			{saveError}
		</div>
	{/if}

	<!-- Footer buttons -->
	<div class="shrink-0 flex justify-between mt-4">
		<button
			type="button"
			class="h-9 px-4 rounded-xl bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 hover:text-base-content transition-colors flex items-center gap-2 disabled:opacity-30"
			onclick={handleReset}
			disabled={isLoading}
		>
			<RotateCcw class="w-4 h-4" />
			{$t('settingsHeartbeat.resetToTemplate')}
		</button>
		<button
			type="button"
			class="h-9 px-5 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all flex items-center gap-2 disabled:opacity-30"
			onclick={handleSave}
			disabled={isSaving || !hasChanges || isLoading}
		>
			{#if isSaving}
				<Loader2 class="w-4 h-4 animate-spin" />
				{$t('common.saving')}
			{:else}
				<Save class="w-4 h-4" />
				{$t('settingsHeartbeat.saveSettings')}
			{/if}
		</button>
	</div>
</div>
