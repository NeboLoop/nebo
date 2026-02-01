<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Save, MapPin, Briefcase, Target, MessageSquare, Clock } from 'lucide-svelte';
	import { getUserProfile, updateUserProfile, type UserProfile } from '$lib/api/nebo';

	let profile = $state<UserProfile | null>(null);
	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let interestsInput = $state('');

	// Form state
	let displayName = $state('');
	let location = $state('');
	let timezone = $state('');
	let occupation = $state('');
	let interests = $state<string[]>([]);
	let communicationStyle = $state('adaptive');
	let goals = $state('');
	let context = $state('');

	// Common timezones
	const timezones = [
		{ value: 'America/New_York', label: 'Eastern Time (US)' },
		{ value: 'America/Chicago', label: 'Central Time (US)' },
		{ value: 'America/Denver', label: 'Mountain Time (US)' },
		{ value: 'America/Los_Angeles', label: 'Pacific Time (US)' },
		{ value: 'America/Phoenix', label: 'Arizona (US)' },
		{ value: 'Europe/London', label: 'London (UK)' },
		{ value: 'Europe/Paris', label: 'Paris (CET)' },
		{ value: 'Europe/Berlin', label: 'Berlin (CET)' },
		{ value: 'Asia/Tokyo', label: 'Tokyo (Japan)' },
		{ value: 'Asia/Shanghai', label: 'Shanghai (China)' },
		{ value: 'Asia/Singapore', label: 'Singapore' },
		{ value: 'Australia/Sydney', label: 'Sydney (Australia)' }
	];

	const communicationStyles = [
		{ value: 'casual', label: 'Casual', description: 'Friendly and informal' },
		{ value: 'professional', label: 'Professional', description: 'Formal and business-like' },
		{ value: 'adaptive', label: 'Adaptive', description: 'Matches your style' }
	];

	onMount(async () => {
		await loadProfile();
	});

	async function loadProfile() {
		isLoading = true;
		try {
			const data = await getUserProfile();
			profile = data.profile;
			if (profile) {
				displayName = profile.displayName || '';
				location = profile.location || '';
				timezone = profile.timezone || '';
				occupation = profile.occupation || '';
				interests = profile.interests || [];
				communicationStyle = profile.communicationStyle || 'adaptive';
				goals = profile.goals || '';
				context = profile.context || '';
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
			await updateUserProfile({
				displayName,
				location,
				timezone,
				occupation,
				interests,
				communicationStyle,
				goals,
				context
			});
			saveMessage = 'Profile saved successfully';
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save profile:', error);
			saveMessage = 'Failed to save profile';
		} finally {
			isSaving = false;
		}
	}

	function addInterest() {
		const newInterest = interestsInput.trim();
		if (newInterest && !interests.includes(newInterest)) {
			interests = [...interests, newInterest];
			interestsInput = '';
		}
	}

	function removeInterest(interest: string) {
		interests = interests.filter((i) => i !== interest);
	}

	function handleInterestKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			addInterest();
		}
	}

	function detectTimezone() {
		const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
		timezone = tz;
	}
</script>

<Card>
	<h2 class="font-display text-xl font-bold text-base-content mb-6">About Me</h2>
	<p class="text-sm text-base-content/60 mb-6">
		Tell the agent about yourself so it can better assist you. This information helps personalize
		your experience.
	</p>

	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading...</div>
	{:else}
		<form
			class="space-y-6"
			onsubmit={(e) => {
				e.preventDefault();
				saveProfile();
			}}
		>
			<!-- Display Name -->
			<div>
				<label class="label" for="display-name">
					<span class="label-text font-medium">What should I call you?</span>
				</label>
				<input
					id="display-name"
					type="text"
					class="input input-bordered w-full max-w-md"
					placeholder="Your name or nickname"
					bind:value={displayName}
				/>
			</div>

			<!-- Location & Timezone -->
			<div class="grid md:grid-cols-2 gap-4">
				<div>
					<label class="label" for="location">
						<span class="label-text font-medium flex items-center gap-2">
							<MapPin class="w-4 h-4" />
							Location
						</span>
					</label>
					<input
						id="location"
						type="text"
						class="input input-bordered w-full"
						placeholder="City, Country"
						bind:value={location}
					/>
				</div>
				<div>
					<label class="label" for="timezone">
						<span class="label-text font-medium flex items-center gap-2">
							<Clock class="w-4 h-4" />
							Timezone
						</span>
					</label>
					<div class="flex gap-2">
						<select id="timezone" class="select select-bordered flex-1" bind:value={timezone}>
							<option value="">Select timezone</option>
							{#each timezones as tz}
								<option value={tz.value}>{tz.label}</option>
							{/each}
						</select>
						<Button type="ghost" size="sm" onclick={detectTimezone}>Detect</Button>
					</div>
				</div>
			</div>

			<!-- Occupation -->
			<div>
				<label class="label" for="occupation">
					<span class="label-text font-medium flex items-center gap-2">
						<Briefcase class="w-4 h-4" />
						What do you do?
					</span>
				</label>
				<input
					id="occupation"
					type="text"
					class="input input-bordered w-full max-w-md"
					placeholder="Your role or profession"
					bind:value={occupation}
				/>
			</div>

			<!-- Interests -->
			<div>
				<label class="label" for="interests">
					<span class="label-text font-medium">Interests & Topics</span>
				</label>
				<div class="flex gap-2 mb-2">
					<input
						id="interests"
						type="text"
						class="input input-bordered flex-1 max-w-md"
						placeholder="Add an interest and press Enter"
						bind:value={interestsInput}
						onkeydown={handleInterestKeydown}
					/>
					<Button type="ghost" onclick={addInterest}>Add</Button>
				</div>
				{#if interests.length > 0}
					<div class="flex flex-wrap gap-2">
						{#each interests as interest}
							<span class="badge badge-lg gap-2">
								{interest}
								<button type="button" class="hover:text-error" onclick={() => removeInterest(interest)}>
									&times;
								</button>
							</span>
						{/each}
					</div>
				{/if}
			</div>

			<!-- Goals -->
			<div>
				<label class="label" for="goals">
					<span class="label-text font-medium flex items-center gap-2">
						<Target class="w-4 h-4" />
						What would you like help with?
					</span>
				</label>
				<textarea
					id="goals"
					class="textarea textarea-bordered w-full"
					rows="3"
					placeholder="What are you trying to accomplish? What do you want the agent to help with most?"
					bind:value={goals}
				></textarea>
			</div>

			<!-- Communication Style -->
			<div>
				<label class="label">
					<span class="label-text font-medium flex items-center gap-2">
						<MessageSquare class="w-4 h-4" />
						Communication Style
					</span>
				</label>
				<div class="grid sm:grid-cols-3 gap-3">
					{#each communicationStyles as style}
						<label
							class="cursor-pointer p-4 rounded-lg border-2 transition-colors {communicationStyle ===
							style.value
								? 'border-primary bg-primary/5'
								: 'border-base-300 hover:border-base-content/20'}"
						>
							<input
								type="radio"
								name="communication-style"
								value={style.value}
								bind:group={communicationStyle}
								class="hidden"
							/>
							<div class="font-medium">{style.label}</div>
							<div class="text-sm text-base-content/60">{style.description}</div>
						</label>
					{/each}
				</div>
			</div>

			<!-- Additional Context -->
			<div>
				<label class="label" for="context">
					<span class="label-text font-medium">Additional Context</span>
					<span class="label-text-alt text-base-content/50">Optional</span>
				</label>
				<textarea
					id="context"
					class="textarea textarea-bordered w-full"
					rows="4"
					placeholder="Anything else the agent should know about you? Preferences, constraints, working style..."
					bind:value={context}
				></textarea>
			</div>

			<!-- Save Button -->
			<div class="flex items-center gap-4 pt-4">
				<Button type="primary" htmlType="submit" disabled={isSaving}>
					<Save class="w-4 h-4 mr-2" />
					{isSaving ? 'Saving...' : 'Save Profile'}
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
