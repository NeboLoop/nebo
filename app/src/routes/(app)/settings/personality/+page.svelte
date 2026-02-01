<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Save, Sparkles, RefreshCw } from 'lucide-svelte';
	import {
		listPersonalityPresets,
		getAgentProfile,
		updateAgentProfile,
		type PersonalityPreset,
		type AgentProfileResponse
	} from '$lib/api/nebo';

	let presets = $state<PersonalityPreset[]>([]);
	let profile = $state<AgentProfileResponse | null>(null);
	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let showCustom = $state(false);

	// Form state
	let selectedPreset = $state('balanced');
	let customPersonality = $state('');
	let voiceStyle = $state('neutral');
	let responseLength = $state('adaptive');
	let emojiUsage = $state('moderate');
	let formality = $state('adaptive');
	let proactivity = $state('moderate');

	const voiceStyles = [
		{ value: 'neutral', label: 'Neutral', description: 'Clear and balanced' },
		{ value: 'warm', label: 'Warm', description: 'Friendly and approachable' },
		{ value: 'professional', label: 'Professional', description: 'Business-focused' },
		{ value: 'enthusiastic', label: 'Enthusiastic', description: 'Energetic and upbeat' }
	];

	const responseLengths = [
		{ value: 'concise', label: 'Concise', description: 'Short and to the point' },
		{ value: 'adaptive', label: 'Adaptive', description: 'Matches the complexity' },
		{ value: 'detailed', label: 'Detailed', description: 'Comprehensive explanations' }
	];

	const emojiLevels = [
		{ value: 'none', label: 'None', description: 'No emojis' },
		{ value: 'minimal', label: 'Minimal', description: 'Occasional use' },
		{ value: 'moderate', label: 'Moderate', description: 'Balanced use' },
		{ value: 'frequent', label: 'Frequent', description: 'Expressive use' }
	];

	const formalityLevels = [
		{ value: 'casual', label: 'Casual', description: 'Relaxed and informal' },
		{ value: 'adaptive', label: 'Adaptive', description: 'Matches context' },
		{ value: 'formal', label: 'Formal', description: 'Professional tone' }
	];

	const proactivityLevels = [
		{ value: 'low', label: 'Reactive', description: 'Only responds when asked' },
		{ value: 'moderate', label: 'Moderate', description: 'Suggests when relevant' },
		{ value: 'high', label: 'Proactive', description: 'Anticipates needs' }
	];

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
			profile = data;
			if (profile) {
				selectedPreset = profile.personalityPreset || 'balanced';
				customPersonality = profile.customPersonality || '';
				voiceStyle = profile.voiceStyle || 'neutral';
				responseLength = profile.responseLength || 'adaptive';
				emojiUsage = profile.emojiUsage || 'moderate';
				formality = profile.formality || 'adaptive';
				proactivity = profile.proactivity || 'moderate';
				showCustom = selectedPreset === 'custom';
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
		try {
			await updateAgentProfile({
				personalityPreset: selectedPreset,
				customPersonality: showCustom ? customPersonality : '',
				voiceStyle,
				responseLength,
				emojiUsage,
				formality,
				proactivity
			});
			saveMessage = 'Personality saved successfully';
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save profile:', error);
			saveMessage = 'Failed to save personality';
		} finally {
			isSaving = false;
		}
	}

	function selectPreset(presetId: string) {
		selectedPreset = presetId;
		showCustom = presetId === 'custom';
	}
</script>

<Card>
	<div class="flex items-center justify-between mb-6">
		<div>
			<h2 class="font-display text-xl font-bold text-base-content">Agent Personality</h2>
			<p class="text-sm text-base-content/60 mt-1">Customize how the agent communicates with you</p>
		</div>
		<Button
			type="ghost"
			onclick={() => {
				loadPresets();
				loadProfile();
			}}
		>
			<RefreshCw class="w-4 h-4" />
		</Button>
	</div>

	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading...</div>
	{:else}
		<form
			class="space-y-8"
			onsubmit={(e) => {
				e.preventDefault();
				saveProfile();
			}}
		>
			<!-- Personality Presets -->
			<div>
				<h3 class="font-medium text-base-content mb-4 flex items-center gap-2">
					<Sparkles class="w-4 h-4" />
					Personality Preset
				</h3>
				<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
					{#each presets as preset}
						<button
							type="button"
							class="text-left p-4 rounded-xl border-2 transition-all hover:shadow-md {selectedPreset ===
							preset.id
								? 'border-primary bg-primary/5 shadow-md'
								: 'border-base-300 hover:border-primary/30'}"
							onclick={() => selectPreset(preset.id)}
						>
							<div class="text-2xl mb-2">{preset.icon}</div>
							<div class="font-bold text-base-content">{preset.name}</div>
							<div class="text-sm text-base-content/60 mt-1">{preset.description}</div>
						</button>
					{/each}
					<!-- Custom option -->
					<button
						type="button"
						class="text-left p-4 rounded-xl border-2 transition-all hover:shadow-md border-dashed {selectedPreset ===
						'custom'
							? 'border-primary bg-primary/5 shadow-md'
							: 'border-base-300 hover:border-primary/30'}"
						onclick={() => selectPreset('custom')}
					>
						<div class="text-2xl mb-2">&#10024;</div>
						<div class="font-bold text-base-content">Custom</div>
						<div class="text-sm text-base-content/60 mt-1">Write your own personality</div>
					</button>
				</div>
			</div>

			<!-- Custom Personality Editor -->
			{#if showCustom}
				<div class="bg-base-200 rounded-xl p-4">
					<label class="label" for="custom-personality">
						<span class="label-text font-medium">Custom Personality Prompt</span>
					</label>
					<textarea
						id="custom-personality"
						class="textarea textarea-bordered w-full"
						rows="6"
						placeholder="Describe how the agent should behave, communicate, and interact with you..."
						bind:value={customPersonality}
					></textarea>
					<p class="text-xs text-base-content/50 mt-2">
						This prompt will be used as the agent's personality. Be specific about tone, style, and
						behavior.
					</p>
				</div>
			{/if}

			<!-- Voice Style -->
			<div>
				<h3 class="font-medium text-base-content mb-4">Voice Style</h3>
				<div class="grid sm:grid-cols-2 lg:grid-cols-4 gap-3">
					{#each voiceStyles as style}
						<label
							class="cursor-pointer p-3 rounded-lg border-2 transition-colors text-center {voiceStyle ===
							style.value
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/20'}"
						>
							<input
								type="radio"
								name="voice-style"
								value={style.value}
								bind:group={voiceStyle}
								class="hidden"
							/>
							<div class="font-medium text-sm">{style.label}</div>
							<div class="text-xs text-base-content/50 mt-1">{style.description}</div>
						</label>
					{/each}
				</div>
			</div>

			<!-- Response Length -->
			<div>
				<h3 class="font-medium text-base-content mb-4">Response Length</h3>
				<div class="grid sm:grid-cols-3 gap-3">
					{#each responseLengths as length}
						<label
							class="cursor-pointer p-3 rounded-lg border-2 transition-colors text-center {responseLength ===
							length.value
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/20'}"
						>
							<input
								type="radio"
								name="response-length"
								value={length.value}
								bind:group={responseLength}
								class="hidden"
							/>
							<div class="font-medium text-sm">{length.label}</div>
							<div class="text-xs text-base-content/50 mt-1">{length.description}</div>
						</label>
					{/each}
				</div>
			</div>

			<!-- Emoji Usage -->
			<div>
				<h3 class="font-medium text-base-content mb-4">Emoji Usage</h3>
				<div class="grid sm:grid-cols-4 gap-3">
					{#each emojiLevels as level}
						<label
							class="cursor-pointer p-3 rounded-lg border-2 transition-colors text-center {emojiUsage ===
							level.value
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/20'}"
						>
							<input
								type="radio"
								name="emoji-usage"
								value={level.value}
								bind:group={emojiUsage}
								class="hidden"
							/>
							<div class="font-medium text-sm">{level.label}</div>
							<div class="text-xs text-base-content/50 mt-1">{level.description}</div>
						</label>
					{/each}
				</div>
			</div>

			<!-- Formality -->
			<div>
				<h3 class="font-medium text-base-content mb-4">Formality Level</h3>
				<div class="grid sm:grid-cols-3 gap-3">
					{#each formalityLevels as level}
						<label
							class="cursor-pointer p-3 rounded-lg border-2 transition-colors text-center {formality ===
							level.value
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/20'}"
						>
							<input
								type="radio"
								name="formality"
								value={level.value}
								bind:group={formality}
								class="hidden"
							/>
							<div class="font-medium text-sm">{level.label}</div>
							<div class="text-xs text-base-content/50 mt-1">{level.description}</div>
						</label>
					{/each}
				</div>
			</div>

			<!-- Proactivity -->
			<div>
				<h3 class="font-medium text-base-content mb-4">Proactivity Level</h3>
				<div class="grid sm:grid-cols-3 gap-3">
					{#each proactivityLevels as level}
						<label
							class="cursor-pointer p-3 rounded-lg border-2 transition-colors text-center {proactivity ===
							level.value
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/20'}"
						>
							<input
								type="radio"
								name="proactivity"
								value={level.value}
								bind:group={proactivity}
								class="hidden"
							/>
							<div class="font-medium text-sm">{level.label}</div>
							<div class="text-xs text-base-content/50 mt-1">{level.description}</div>
						</label>
					{/each}
				</div>
			</div>

			<!-- Save Button -->
			<div class="flex items-center gap-4 pt-4 border-t border-base-300">
				<Button type="primary" htmlType="submit" disabled={isSaving}>
					<Save class="w-4 h-4 mr-2" />
					{isSaving ? 'Saving...' : 'Save Personality'}
				</Button>
				{#if saveMessage}
					<span class="text-sm {saveMessage.includes('success') ? 'text-success' : 'text-error'}">
						{saveMessage}
					</span>
				{/if}
			</div>
		</form>
	{/if}
</Card>
