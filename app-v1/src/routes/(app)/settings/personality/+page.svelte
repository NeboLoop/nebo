<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { Save } from 'lucide-svelte';
	import {
		listPersonalityPresets,
		getAgentProfile,
		updateAgentProfile,
		type PersonalityPreset
	} from '$lib/api/nebo';

	let presets = $state<PersonalityPreset[]>([]);
	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let saveError = $state(false);

	let agentName = $state('Nebo');
	let selectedPreset = $state('balanced');
	let customPersonality = $state('');
	let voiceStyle = $state('neutral');
	let responseLength = $state('adaptive');
	let emojiUsage = $state('moderate');
	let formality = $state('adaptive');
	let proactivity = $state('moderate');

	const tuningRows = $derived([
		{
			labelKey: 'settingsPersonality.voice',
			options: [
				{ value: 'neutral', labelKey: 'settingsPersonality.voiceOptions.neutral' },
				{ value: 'warm', labelKey: 'settingsPersonality.voiceOptions.warm' },
				{ value: 'professional', labelKey: 'settingsPersonality.voiceOptions.professional' },
				{ value: 'enthusiastic', labelKey: 'settingsPersonality.voiceOptions.enthusiastic' }
			],
			value: voiceStyle,
			set: (v: string) => (voiceStyle = v)
		},
		{
			labelKey: 'settingsPersonality.length',
			options: [
				{ value: 'concise', labelKey: 'settingsPersonality.lengthOptions.concise' },
				{ value: 'adaptive', labelKey: 'settingsPersonality.lengthOptions.adaptive' },
				{ value: 'detailed', labelKey: 'settingsPersonality.lengthOptions.detailed' }
			],
			value: responseLength,
			set: (v: string) => (responseLength = v)
		},
		{
			labelKey: 'settingsPersonality.emojis',
			options: [
				{ value: 'none', labelKey: 'settingsPersonality.emojiOptions.none' },
				{ value: 'minimal', labelKey: 'settingsPersonality.emojiOptions.minimal' },
				{ value: 'moderate', labelKey: 'settingsPersonality.emojiOptions.moderate' },
				{ value: 'frequent', labelKey: 'settingsPersonality.emojiOptions.frequent' }
			],
			value: emojiUsage,
			set: (v: string) => (emojiUsage = v)
		},
		{
			labelKey: 'settingsPersonality.formality',
			options: [
				{ value: 'casual', labelKey: 'settingsPersonality.formalityOptions.casual' },
				{ value: 'adaptive', labelKey: 'settingsPersonality.formalityOptions.adaptive' },
				{ value: 'formal', labelKey: 'settingsPersonality.formalityOptions.formal' }
			],
			value: formality,
			set: (v: string) => (formality = v)
		},
		{
			labelKey: 'settingsPersonality.proactivity',
			options: [
				{ value: 'low', labelKey: 'settingsPersonality.proactivityOptions.reactive' },
				{ value: 'moderate', labelKey: 'settingsPersonality.proactivityOptions.moderate' },
				{ value: 'high', labelKey: 'settingsPersonality.proactivityOptions.proactive' }
			],
			value: proactivity,
			set: (v: string) => (proactivity = v)
		}
	]);

	onMount(async () => {
		await loadPresets();
		await loadProfile();
	});

	async function loadPresets() {
		try {
			const data = await listPersonalityPresets();
			presets = data.presets || [];
		} catch (error) {
			console.error('Failed to load presets:', error);
		}
	}

	async function loadProfile() {
		isLoading = true;
		try {
			const data = await getAgentProfile();
			if (data) {
				agentName = data.name || 'Nebo';
				selectedPreset = data.personalityPreset || 'balanced';
				customPersonality = data.customPersonality || '';
				voiceStyle = data.voiceStyle || 'neutral';
				responseLength = data.responseLength || 'adaptive';
				emojiUsage = data.emojiUsage || 'moderate';
				formality = data.formality || 'adaptive';
				proactivity = data.proactivity || 'moderate';

				// If no custom personality saved, seed from the active preset
				if (!customPersonality) {
					const preset = presets.find((p) => p.id === selectedPreset);
					if (preset) customPersonality = preset.systemPrompt;
				}
				// Store the loaded personality as the default for revert
				defaultPersonality = customPersonality;
			}
		} catch (error) {
			console.error('Failed to load profile:', error);
		} finally {
			isLoading = false;
		}
	}

	async function saveProfile() {
		isSaving = true;
		saveMessage = '';
		saveError = false;
		try {
			await updateAgentProfile({
				name: agentName,
				personalityPreset: selectedPreset,
				customPersonality,
				voiceStyle,
				responseLength,
				emojiUsage,
				formality,
				proactivity
			});
			saveMessage = $t('settingsPersonality.soulSaved');
			saveError = false;
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save profile:', error);
			saveMessage = $t('settingsPersonality.saveFailed');
			saveError = true;
		} finally {
			isSaving = false;
		}
	}

	let previousPersonality = $state('');
	let defaultPersonality = $state('');

	function loadPreset(e: Event) {
		const select = e.target as HTMLSelectElement;
		const presetId = select.value;
		if (!presetId) return;

		const preset = presets.find((p) => p.id === presetId);
		if (preset) {
			previousPersonality = customPersonality;
			selectedPreset = presetId;
			customPersonality = preset.systemPrompt;
			showRevert = true;
		}
		select.value = '';
	}

	let showRevert = $state(false);

	const hasChangedFromDefault = $derived(
		defaultPersonality !== '' && customPersonality !== defaultPersonality && !showRevert
	);

	function revertSoul() {
		customPersonality = previousPersonality;
		previousPersonality = '';
		showRevert = false;
	}

	function revertToDefault() {
		customPersonality = defaultPersonality;
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsPersonality.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsPersonality.description')}</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsPersonality.loadingPersonality')}</span>
	</div>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			saveProfile();
		}}
		class="space-y-6"
	>
		<!-- Soul -->
		<section>
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">{$t('settingsPersonality.soulSection')}</h3>
				{#if presets.length > 0}
					<select
						class="h-8 rounded-lg bg-base-content/5 border border-base-content/10 px-3 text-base focus:outline-none focus:border-primary/50 transition-colors"
						onchange={loadPreset}
					>
						<option value="" selected disabled>{$t('settingsPersonality.loadTemplate')}</option>
						{#each presets.filter((p) => p.id !== 'custom') as preset}
							<option value={preset.id}>{preset.icon} {preset.name}</option>
						{/each}
					</select>
				{/if}
			</div>

			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<textarea
					id="personality-prompt"
					class="w-full rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 font-mono text-base leading-relaxed resize-none overflow-y-auto focus:outline-none focus:border-primary/50 transition-colors"
					style="min-height: 6rem; max-height: 60vh; field-sizing: content;"
					placeholder={$t('settingsPersonality.placeholder')}
					bind:value={customPersonality}
				></textarea>
				{#if showRevert}
					<div class="flex items-center justify-between mt-3 px-4 py-2.5 rounded-xl bg-base-content/5 border border-base-content/10">
						<span class="text-base text-base-content/80">{$t('settingsPersonality.templateLoaded')}</span>
						<button type="button" class="text-base font-medium text-primary hover:text-primary/80 transition-colors" onclick={revertSoul}>
							{$t('settingsPersonality.undo')}
						</button>
					</div>
				{:else if hasChangedFromDefault}
					<div class="flex items-center justify-between mt-3">
						<p class="text-base text-base-content/80">
							{$t('settingsPersonality.hint')}
						</p>
						<button type="button" class="text-sm font-medium text-base-content/50 hover:text-base-content/80 transition-colors" onclick={revertToDefault}>
							{$t('settingsPersonality.revert')}
						</button>
					</div>
				{:else}
					<p class="text-base text-base-content/80 mt-2">
						{$t('settingsPersonality.hint')}
					</p>
				{/if}
			</div>
		</section>

		<!-- Tuning -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsPersonality.tuning')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-4">
				{#each tuningRows as row}
					<div class="flex flex-col sm:flex-row sm:items-center gap-1.5 sm:gap-3">
						<span class="text-base font-medium text-base-content w-24 shrink-0">{$t(row.labelKey)}</span>
						<div class="flex flex-wrap gap-1.5">
							{#each row.options as option}
								<button
									type="button"
									class="px-3 py-1.5 rounded-lg text-base font-medium border transition-all
										{row.value === option.value
											? 'bg-primary/10 border-primary/30 text-primary'
											: 'bg-base-content/5 border-transparent text-base-content/90 hover:border-base-content/15'}"
									onclick={() => row.set(option.value)}
								>
									{$t(option.labelKey)}
								</button>
							{/each}
						</div>
					</div>
				{/each}
			</div>
		</section>

		<!-- Save -->
		{#if saveMessage}
			<div class="rounded-xl {saveError ? 'bg-error/10 border border-error/20 text-error' : 'bg-success/10 border border-success/20 text-success'} px-4 py-3 text-base">
				{saveMessage}
			</div>
		{/if}

		<div class="flex justify-end">
			<button
				type="submit"
				disabled={isSaving}
				class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
			>
				{#if isSaving}
					<Spinner size={16} />
					{$t('common.saving')}
				{:else}
					{$t('settingsPersonality.saveSoul')}
				{/if}
			</button>
		</div>
	</form>
{/if}
