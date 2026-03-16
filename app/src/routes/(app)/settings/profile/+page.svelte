<script lang="ts">
	import { onMount } from 'svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import {
		X,
		Plus,
		Sun,
		Moon,
		Monitor
	} from 'lucide-svelte';
	import { getUserProfile, updateUserProfile, type UserProfile } from '$lib/api/nebo';
	import * as api from '$lib/api/nebo';

	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveMessage = $state('');
	let saveError = $state(false);
	let interestsInput = $state('');

	type Theme = 'light' | 'dark' | 'system';
	let theme = $state<Theme>('dark');
	let themeError = $state('');

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

	const themeOptions = [
		{ id: 'light' as Theme, label: 'Light', icon: Sun },
		{ id: 'dark' as Theme, label: 'Dark', icon: Moon },
		{ id: 'system' as Theme, label: 'System', icon: Monitor }
	];

	const communicationStyles = [
		{ value: 'casual', label: 'Casual', description: 'Friendly and informal' },
		{ value: 'professional', label: 'Professional', description: 'Structured and precise' },
		{ value: 'adaptive', label: 'Adaptive', description: 'Mirrors your tone' }
	];

	onMount(async () => {
		try {
			const [profileData, prefsData] = await Promise.all([
				getUserProfile(),
				api.getPreferences()
			]);
			const profile = profileData.profile;
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
			theme = (prefsData.preferences?.theme as Theme) || 'dark';
		} catch (error) {
			console.error('Failed to load profile:', error);
		} finally {
			isLoading = false;
		}
	});

	async function setTheme(newTheme: Theme) {
		theme = newTheme;
		themeError = '';
		if (typeof document !== 'undefined') {
			if (newTheme === 'system') {
				const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
				document.documentElement.setAttribute('data-theme', prefersDark ? 'dark' : 'light');
			} else {
				document.documentElement.setAttribute('data-theme', newTheme);
			}
		}
		try {
			await api.updatePreferences({ theme: newTheme, emailNotifications: false, marketingEmails: false });
		} catch (err: any) {
			themeError = err?.message || 'Failed to save theme';
			setTimeout(() => { themeError = ''; }, 4000);
		}
	}

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

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Profile</h2>
	<p class="text-base text-base-content/80">Your preferences and personal information</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">Loading profile...</span>
	</div>
{:else}
	<form
		onsubmit={(e) => {
			e.preventDefault();
			saveProfile();
		}}
		class="space-y-6"
	>
		<!-- Appearance -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Appearance</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex gap-2" role="group" aria-label="Theme selection">
					{#each themeOptions as option}
						<button
							type="button"
							onclick={() => setTheme(option.id)}
							class="flex-1 flex items-center justify-center gap-2 h-10 rounded-xl border transition-all
								{theme === option.id
									? 'bg-primary/10 border-primary/30 text-primary'
									: 'bg-base-content/5 border-transparent text-base-content/90 hover:border-base-content/15'}"
						>
							<option.icon class="w-4 h-4" />
							<span class="text-base font-medium">{option.label}</span>
						</button>
					{/each}
				</div>
				{#if themeError}
					<p class="text-base text-error mt-2">{themeError}</p>
				{/if}
			</div>
		</section>

		<!-- About You -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">About You</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				<div>
					<label class="text-base font-medium text-base-content/80" for="display-name">
						What should I call you?
					</label>
					<input
						id="display-name"
						type="text"
						class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						placeholder="Your name or nickname"
						bind:value={displayName}
					/>
				</div>

				<div class="grid sm:grid-cols-2 gap-4">
					<div>
						<label class="text-base font-medium text-base-content/80" for="occupation">
							What do you do?
						</label>
						<input
							id="occupation"
							type="text"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							placeholder="Your role or profession"
							bind:value={occupation}
						/>
					</div>
					<div>
						<label class="text-base font-medium text-base-content/80" for="location">
							Location
						</label>
						<input
							id="location"
							type="text"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							placeholder="City, Country"
							bind:value={location}
						/>
					</div>
				</div>

				<div>
					<label class="text-base font-medium text-base-content/80" for="timezone">
						Timezone
					</label>
					<div class="flex gap-2 mt-2">
						<select
							id="timezone"
							class="select flex-1"
							bind:value={timezone}
						>
							<option value="">Select timezone</option>
							{#each timezones as tz}
								<option value={tz.value}>{tz.label}</option>
							{/each}
						</select>
						<button
							type="button"
							class="h-11 px-4 rounded-xl bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 transition-colors"
							onclick={detectTimezone}
						>
							Detect
						</button>
					</div>
				</div>
			</div>
		</section>

		<!-- Interests -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-1">Interests</h3>
			<p class="text-base text-base-content/80 mb-3">Topics you care about — the agent will tailor responses accordingly</p>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex gap-2">
					<input
						type="text"
						class="flex-1 h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						placeholder="Type an interest and press Enter"
						bind:value={interestsInput}
						onkeydown={handleInterestKeydown}
					/>
					<button
						type="button"
						class="w-11 h-11 rounded-xl bg-base-content/5 border border-base-content/10 flex items-center justify-center hover:border-base-content/40 transition-colors disabled:opacity-30"
						onclick={addInterest}
						disabled={!interestsInput.trim()}
						aria-label="Add interest"
					>
						<Plus class="w-4 h-4 text-base-content/90" />
					</button>
				</div>
				{#if interests.length > 0}
					<div class="flex flex-wrap gap-1.5 mt-4">
						{#each interests as interest}
							<span class="inline-flex items-center gap-1 px-3 py-1.5 rounded-full bg-base-content/5 border border-base-content/10 text-base">
								{interest}
								<button
									type="button"
									class="ml-0.5 p-0.5 rounded-full hover:bg-base-content/10 transition-colors"
									onclick={() => removeInterest(interest)}
									aria-label="Remove {interest}"
								>
									<X class="w-3 h-3 text-base-content/90" />
								</button>
							</span>
						{/each}
					</div>
				{:else}
					<p class="text-base text-base-content/80 mt-3">No interests added yet</p>
				{/if}
			</div>
		</section>

		<!-- Goals & Context -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Goals & Context</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				<div>
					<label class="text-base font-medium text-base-content/80" for="goals">
						What would you like help with?
					</label>
					<textarea
						id="goals"
						class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base focus:outline-none focus:border-primary/50 transition-colors resize-none"
						rows="3"
						placeholder="What are you trying to accomplish?"
						bind:value={goals}
					></textarea>
				</div>

				<div>
					<label class="text-base font-medium text-base-content/80" for="context">
						Additional context
						<span class="font-normal text-base-content/90 ml-1">optional</span>
					</label>
					<textarea
						id="context"
						class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base focus:outline-none focus:border-primary/50 transition-colors resize-none"
						rows="3"
						placeholder="Preferences, constraints, working style, things to avoid..."
						bind:value={context}
					></textarea>
				</div>
			</div>
		</section>

		<!-- Communication Style -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">Communication Style</h3>
			<div class="grid sm:grid-cols-3 gap-2">
				{#each communicationStyles as style}
					<label
						class="cursor-pointer rounded-xl border p-4 transition-all
							{communicationStyle === style.value
								? 'bg-primary/10 border-primary/30'
								: 'bg-base-200/50 border-base-content/10 hover:border-base-content/40'}"
					>
						<input
							type="radio"
							name="communication-style"
							value={style.value}
							bind:group={communicationStyle}
							class="hidden"
						/>
						<div class="font-medium text-base">{style.label}</div>
						<div class="text-base text-base-content/80 mt-0.5">{style.description}</div>
					</label>
				{/each}
			</div>
		</section>

		<!-- Save -->
		{#if saveMessage}
			<Alert type={saveError ? 'error' : 'success'} title={saveError ? 'Error' : 'Saved'}>
				{saveMessage}
			</Alert>
		{/if}

		<div class="flex justify-end">
			<button
				type="submit"
				disabled={isSaving}
				class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
			>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Profile
				{/if}
			</button>
		</div>
	</form>
{/if}
