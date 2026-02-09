<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import {
		Save,
		MapPin,
		Briefcase,
		Clock,
		X,
		Plus
	} from 'lucide-svelte';
	import { getUserProfile, updateUserProfile, type UserProfile } from '$lib/api/nebo';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let saveError = $state(false);
	let interestsInput = $state('');

	let displayName = $state('');
	let location = $state('');
	let timezone = $state('');
	let occupation = $state('');
	let interests = $state<string[]>([]);
	let communicationStyle = $state('adaptive');
	let goals = $state('');
	let context = $state('');

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
		{ value: 'professional', label: 'Professional', description: 'Structured and precise' },
		{ value: 'adaptive', label: 'Adaptive', description: 'Mirrors your tone' }
	];

	onMount(async () => {
		try {
			const data = await getUserProfile();
			const profile = data.profile;
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
	});

	async function saveProfile() {
		isSaving = true;
		saveMessage = '';
		saveError = false;
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
			saveMessage = 'Profile saved';
			saveError = false;
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save profile:', error);
			saveMessage = 'Failed to save profile';
			saveError = true;
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
		timezone = Intl.DateTimeFormat().resolvedOptions().timeZone;
	}
</script>

{#if isLoading}
	<Card>
		<div class="flex items-center justify-center gap-3 py-8">
			<Spinner size={20} />
			<span class="text-sm text-base-content/60">Loading profile...</span>
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
			<!-- About You -->
			<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-4">About You</h3>

			<div class="space-y-4">
				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="display-name">
						What should I call you?
					</label>
					<input
						id="display-name"
						type="text"
						class="input input-bordered input-sm w-full max-w-sm"
						placeholder="Your name or nickname"
						bind:value={displayName}
					/>
				</div>

				<div class="grid sm:grid-cols-2 gap-3">
					<div>
						<label class="block text-sm font-medium text-base-content mb-1" for="occupation">
							What do you do?
						</label>
						<input
							id="occupation"
							type="text"
							class="input input-bordered input-sm w-full"
							placeholder="Your role or profession"
							bind:value={occupation}
						/>
					</div>
					<div>
						<label class="block text-sm font-medium text-base-content mb-1" for="location">
							Location
						</label>
						<input
							id="location"
							type="text"
							class="input input-bordered input-sm w-full"
							placeholder="City, Country"
							bind:value={location}
						/>
					</div>
				</div>

				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="timezone">
						Timezone
					</label>
					<div class="flex gap-2">
						<select id="timezone" class="select select-bordered select-sm flex-1 max-w-sm" bind:value={timezone}>
							<option value="">Select timezone</option>
							{#each timezones as tz}
								<option value={tz.value}>{tz.label}</option>
							{/each}
						</select>
						<button type="button" class="btn btn-ghost btn-sm text-xs" onclick={detectTimezone}>Detect</button>
					</div>
				</div>
			</div>

			<div class="divider"></div>

			<!-- Interests -->
			<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-3">Interests</h3>
			<p class="text-xs text-base-content/40 mb-3">Topics you care about — the agent will tailor responses accordingly</p>

			<div class="flex gap-2 mb-3">
				<input
					type="text"
					class="input input-bordered input-sm flex-1 max-w-sm"
					placeholder="Type an interest and press Enter"
					bind:value={interestsInput}
					onkeydown={handleInterestKeydown}
				/>
				<button
					type="button"
					class="btn btn-ghost btn-sm btn-square"
					onclick={addInterest}
					disabled={!interestsInput.trim()}
				>
					<Plus class="w-3.5 h-3.5" />
				</button>
			</div>
			{#if interests.length > 0}
				<div class="flex flex-wrap gap-1.5">
					{#each interests as interest}
						<span class="badge badge-outline gap-1 pr-1">
							{interest}
							<button
								type="button"
								class="btn btn-ghost btn-xs btn-circle"
								onclick={() => removeInterest(interest)}
							>
								<X class="w-3 h-3" />
							</button>
						</span>
					{/each}
				</div>
			{:else}
				<p class="text-xs text-base-content/30">No interests added yet</p>
			{/if}

			<div class="divider"></div>

			<!-- Goals -->
			<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-3">Goals & Context</h3>

			<div class="space-y-3">
				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="goals">
						What would you like help with?
					</label>
					<textarea
						id="goals"
						class="textarea textarea-bordered textarea-sm w-full"
						rows="2"
						placeholder="What are you trying to accomplish?"
						bind:value={goals}
					></textarea>
				</div>

				<div>
					<label class="block text-sm font-medium text-base-content mb-1" for="context">
						Additional context
						<span class="font-normal text-base-content/30 ml-1">optional</span>
					</label>
					<textarea
						id="context"
						class="textarea textarea-bordered textarea-sm w-full"
						rows="2"
						placeholder="Preferences, constraints, working style, things to avoid..."
						bind:value={context}
					></textarea>
				</div>
			</div>

			<div class="divider"></div>

			<!-- Communication Style -->
			<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-3">Communication Style</h3>

			<div class="grid sm:grid-cols-3 gap-2">
				{#each communicationStyles as style}
					<label
						class="cursor-pointer rounded-lg border-2 p-3 transition-all
							{communicationStyle === style.value
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
						<div class="font-medium text-sm">{style.label}</div>
						<div class="text-xs text-base-content/50">{style.description}</div>
					</label>
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
					Save Profile
				{/if}
			</Button>
		</div>
	</form>
{/if}
