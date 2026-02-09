<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
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
			label: 'Voice',
			options: [
				{ value: 'neutral', label: 'Neutral' },
				{ value: 'warm', label: 'Warm' },
				{ value: 'professional', label: 'Professional' },
				{ value: 'enthusiastic', label: 'Enthusiastic' }
			],
			value: voiceStyle,
			set: (v: string) => (voiceStyle = v)
		},
		{
			label: 'Length',
			options: [
				{ value: 'concise', label: 'Concise' },
				{ value: 'adaptive', label: 'Adaptive' },
				{ value: 'detailed', label: 'Detailed' }
			],
			value: responseLength,
			set: (v: string) => (responseLength = v)
		},
		{
			label: 'Emojis',
			options: [
				{ value: 'none', label: 'None' },
				{ value: 'minimal', label: 'Minimal' },
				{ value: 'moderate', label: 'Moderate' },
				{ value: 'frequent', label: 'Frequent' }
			],
			value: emojiUsage,
			set: (v: string) => (emojiUsage = v)
		},
		{
			label: 'Formality',
			options: [
				{ value: 'casual', label: 'Casual' },
				{ value: 'adaptive', label: 'Adaptive' },
				{ value: 'formal', label: 'Formal' }
			],
			value: formality,
			set: (v: string) => (formality = v)
		},
		{
			label: 'Proactivity',
			options: [
				{ value: 'low', label: 'Reactive' },
				{ value: 'moderate', label: 'Moderate' },
				{ value: 'high', label: 'Proactive' }
			],
			value: proactivity,
			set: (v: string) => (proactivity = v)
		}
	]);

	onMount(async () => {
		await Promise.all([loadPresets(), loadProfile()]);
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
			saveMessage = 'Personality saved';
			saveError = false;
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save profile:', error);
			saveMessage = 'Failed to save';
			saveError = true;
		} finally {
			isSaving = false;
		}
	}

	let previousPersonality = $state('');

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

	function revertSoul() {
		customPersonality = previousPersonality;
		previousPersonality = '';
		showRevert = false;
	}
</script>

{#if isLoading}
	<Card>
		<div class="flex items-center justify-center gap-3 py-8">
			<Spinner size={20} />
			<span class="text-sm text-base-content/60">Loading personality...</span>
		</div>
	</Card>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			saveProfile();
		}}
	>
		<Card>
			<!-- Soul -->
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider">Soul</h3>
				{#if presets.length > 0}
					<select
						class="select select-bordered select-xs text-xs"
						onchange={loadPreset}
					>
						<option value="" selected disabled>Load a template...</option>
						{#each presets.filter((p) => p.id !== 'custom') as preset}
							<option value={preset.id}>{preset.icon} {preset.name}</option>
						{/each}
					</select>
				{/if}
			</div>

			<textarea
				id="personality-prompt"
				class="textarea textarea-bordered w-full font-mono text-xs leading-relaxed resize-none overflow-y-auto"
				style="min-height: 6rem; max-height: 60vh; field-sizing: content;"
				placeholder="Define your agent's personality, behavior, and communication style..."
				bind:value={customPersonality}
			></textarea>
			{#if showRevert}
				<div class="flex items-center justify-between mt-2 px-3 py-2 rounded-lg bg-base-200">
					<span class="text-sm text-base-content/70">Template loaded — replaced your previous soul.</span>
					<button type="button" class="btn btn-ghost btn-xs" onclick={revertSoul}>
						Undo
					</button>
				</div>
			{:else}
				<p class="text-xs text-base-content/30 mt-1">
					This is your agent's core personality prompt — its soul.
				</p>
			{/if}

			<div class="divider"></div>

			<!-- Tuning -->
			<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-3">Tuning</h3>

			<div class="space-y-3">
				{#each tuningRows as row}
					<div class="flex flex-col sm:flex-row sm:items-center gap-1.5 sm:gap-3">
						<span class="text-sm font-medium text-base-content w-24 shrink-0">{row.label}</span>
						<div class="flex flex-wrap gap-1.5">
							{#each row.options as option}
								<button
									type="button"
									class="px-2.5 py-1 rounded-md text-xs font-medium border transition-colors
										{row.value === option.value
											? 'bg-primary/10 border-primary/30 text-primary'
											: 'bg-base-200 border-transparent text-base-content/50 hover:border-base-content/15'}"
									onclick={() => row.set(option.value)}
								>
									{option.label}
								</button>
							{/each}
						</div>
					</div>
				{/each}
			</div>
		</Card>

		{#if saveMessage}
			<div class="mt-4">
				<Alert type={saveError ? 'error' : 'success'} title={saveError ? 'Error' : 'Saved'}>
					{saveMessage}
				</Alert>
			</div>
		{/if}

		<div class="flex justify-end mt-4">
			<Button type="primary" htmlType="submit" disabled={isSaving}>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Personality
				{/if}
			</Button>
		</div>
	</form>
{/if}
