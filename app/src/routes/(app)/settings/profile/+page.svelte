<script lang="ts">
	import { onMount } from 'svelte';
	import { t, locale } from 'svelte-i18n';
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
	let theme = $state<Theme>('system');
	let themeError = $state('');

	const languages = [
		{ value: 'en', label: 'English' },
		{ value: 'id', label: 'Bahasa Indonesia' },
		{ value: 'ms', label: 'Bahasa Melayu' },
		{ value: 'de', label: 'Deutsch' },
		{ value: 'es', label: 'Español' },
		{ value: 'fr', label: 'Français' },
		{ value: 'it', label: 'Italiano' },
		{ value: 'nl', label: 'Nederlands' },
		{ value: 'pl', label: 'Polski' },
		{ value: 'pt', label: 'Português' },
		{ value: 'pt-BR', label: 'Português (Brasil)' },
		{ value: 'sv', label: 'Svenska' },
		{ value: 'vi', label: 'Tiếng Việt' },
		{ value: 'tr', label: 'Türkçe' },
		{ value: 'ru', label: 'Русский' },
		{ value: 'uk', label: 'Українська' },
		{ value: 'ar', label: 'العربية' },
		{ value: 'he', label: 'עברית' },
		{ value: 'bn', label: 'বাংলা' },
		{ value: 'hi', label: 'हिन्दी' },
		{ value: 'th', label: 'ไทย' },
		{ value: 'zh-CN', label: '中文 (简体)' },
		{ value: 'zh-TW', label: '中文 (繁體)' },
		{ value: 'ja', label: '日本語' },
		{ value: 'ko', label: '한국어' }
	];

	const supportedLocales = languages.map(l => l.value);

	function detectSystemLanguage(): string {
		const browserLang = navigator.language;
		if (supportedLocales.includes(browserLang)) return browserLang;
		const base = browserLang.split('-')[0];
		const match = supportedLocales.find(l => l === base || l.startsWith(base + '-'));
		return match ?? 'en';
	}

	let language = $state('en');

	let displayName = $state('');
	let location = $state('');
	let timezone = $state('');
	let occupation = $state('');
	let interests = $state<string[]>([]);
	let communicationStyle = $state('adaptive');
	let goals = $state('');
	let context = $state('');

	const timezones = [
		{ value: 'America/New_York', labelKey: 'settingsProfile.timezones.eastern' },
		{ value: 'America/Chicago', labelKey: 'settingsProfile.timezones.central' },
		{ value: 'America/Denver', labelKey: 'settingsProfile.timezones.mountain' },
		{ value: 'America/Los_Angeles', labelKey: 'settingsProfile.timezones.pacific' },
		{ value: 'America/Phoenix', labelKey: 'settingsProfile.timezones.arizona' },
		{ value: 'Europe/London', labelKey: 'settingsProfile.timezones.london' },
		{ value: 'Europe/Paris', labelKey: 'settingsProfile.timezones.paris' },
		{ value: 'Europe/Berlin', labelKey: 'settingsProfile.timezones.berlin' },
		{ value: 'Asia/Tokyo', labelKey: 'settingsProfile.timezones.tokyo' },
		{ value: 'Asia/Shanghai', labelKey: 'settingsProfile.timezones.shanghai' },
		{ value: 'Asia/Singapore', labelKey: 'settingsProfile.timezones.singapore' },
		{ value: 'Australia/Sydney', labelKey: 'settingsProfile.timezones.sydney' }
	];

	const themeOptions = [
		{ id: 'light' as Theme, labelKey: 'theme.light', icon: Sun },
		{ id: 'dark' as Theme, labelKey: 'theme.dark', icon: Moon },
		{ id: 'system' as Theme, labelKey: 'theme.system', icon: Monitor }
	];

	const communicationStyles = [
		{ value: 'casual', labelKey: 'settingsProfile.casual', descriptionKey: 'settingsProfile.casualDesc' },
		{ value: 'professional', labelKey: 'settingsProfile.professional', descriptionKey: 'settingsProfile.professionalDesc' },
		{ value: 'adaptive', labelKey: 'settingsProfile.adaptive', descriptionKey: 'settingsProfile.adaptiveDesc' }
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
			theme = (prefsData.preferences?.theme as Theme) || 'system';
			language = prefsData.preferences?.language || detectSystemLanguage();
			locale.set(language);
			localStorage.setItem('nebo_locale', language);
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
			await api.updatePreferences({ theme: newTheme });
		} catch (err: any) {
			themeError = err?.message || $t('settingsProfile.themeSaveFailed');
			setTimeout(() => { themeError = ''; }, 4000);
		}
	}

	async function setLanguage(newLang: string) {
		language = newLang;
		locale.set(newLang);
		localStorage.setItem('nebo_locale', newLang);
		if (typeof document !== 'undefined') {
			document.documentElement.dir = (newLang === 'ar' || newLang === 'he') ? 'rtl' : 'ltr';
			document.documentElement.lang = newLang;
		}
		try {
			await api.updatePreferences({ language: newLang });
		} catch (err: any) {
			console.error('Failed to save language:', err);
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
			saveMessage = $t('settingsProfile.profileSaved');
			saveError = false;
			setTimeout(() => (saveMessage = ''), 3000);
		} catch (error) {
			console.error('Failed to save profile:', error);
			saveMessage = $t('settingsProfile.saveFailed');
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
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsProfile.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsProfile.description')}</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsProfile.loadingProfile')}</span>
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
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProfile.appearance')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex gap-2" role="group" aria-label={$t('settingsProfile.themeSelection')}>
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
							<span class="text-base font-medium">{$t(option.labelKey)}</span>
						</button>
					{/each}
				</div>
				{#if themeError}
					<p class="text-base text-error mt-2">{themeError}</p>
				{/if}
			</div>
		</section>

		<!-- Language -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProfile.language')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<label class="text-base font-medium text-base-content/80" for="language-select">
					{$t('settingsProfile.languageLabel')}
				</label>
				<select
					id="language-select"
					class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
					bind:value={language}
					onchange={() => setLanguage(language)}
				>
					{#each languages as lang}
						<option value={lang.value}>{lang.label}</option>
					{/each}
				</select>
			</div>
		</section>

		<!-- About You -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProfile.aboutYou')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				<div>
					<label class="text-base font-medium text-base-content/80" for="display-name">
						{$t('settingsProfile.nameLabel')}
					</label>
					<input
						id="display-name"
						type="text"
						class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						placeholder={$t('settingsProfile.namePlaceholder')}
						bind:value={displayName}
					/>
				</div>

				<div class="grid sm:grid-cols-2 gap-4">
					<div>
						<label class="text-base font-medium text-base-content/80" for="occupation">
							{$t('settingsProfile.roleLabel')}
						</label>
						<input
							id="occupation"
							type="text"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							placeholder={$t('settingsProfile.rolePlaceholder')}
							bind:value={occupation}
						/>
					</div>
					<div>
						<label class="text-base font-medium text-base-content/80" for="location">
							{$t('settingsProfile.locationLabel')}
						</label>
						<input
							id="location"
							type="text"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							placeholder={$t('settingsProfile.locationPlaceholder')}
							bind:value={location}
						/>
					</div>
				</div>

				<div>
					<label class="text-base font-medium text-base-content/80" for="timezone">
						{$t('settingsProfile.timezoneLabel')}
					</label>
					<div class="flex gap-2 mt-2">
						<select
							id="timezone"
							class="flex-1 h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							bind:value={timezone}
						>
							<option value="">{$t('settingsProfile.timezonePlaceholder')}</option>
							{#each timezones as tz}
								<option value={tz.value}>{$t(tz.labelKey)}</option>
							{/each}
						</select>
						<button
							type="button"
							class="h-11 px-4 rounded-xl bg-base-content/5 border border-base-content/10 text-base font-medium text-base-content/80 hover:border-base-content/40 transition-colors"
							onclick={detectTimezone}
						>
							{$t('settingsProfile.detect')}
						</button>
					</div>
				</div>
			</div>
		</section>

		<!-- Interests -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-1">{$t('settingsProfile.interests')}</h3>
			<p class="text-base text-base-content/80 mb-3">{$t('settingsProfile.interestsHint')}</p>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				<div class="flex gap-2">
					<input
						type="text"
						class="flex-1 h-11 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						placeholder={$t('settingsProfile.interestsPlaceholder')}
						bind:value={interestsInput}
						onkeydown={handleInterestKeydown}
					/>
					<button
						type="button"
						class="w-11 h-11 rounded-xl bg-base-content/5 border border-base-content/10 flex items-center justify-center hover:border-base-content/40 transition-colors disabled:opacity-30"
						onclick={addInterest}
						disabled={!interestsInput.trim()}
						aria-label={$t('settingsProfile.addInterest')}
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
									aria-label={$t('settingsProfile.removeInterest', { values: { interest } })}
								>
									<X class="w-3 h-3 text-base-content/90" />
								</button>
							</span>
						{/each}
					</div>
				{:else}
					<p class="text-base text-base-content/80 mt-3">{$t('settingsProfile.noInterests')}</p>
				{/if}
			</div>
		</section>

		<!-- Goals & Context -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProfile.goalsContext')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5 space-y-5">
				<div>
					<label class="text-base font-medium text-base-content/80" for="goals">
						{$t('settingsProfile.goalsLabel')}
					</label>
					<textarea
						id="goals"
						class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base focus:outline-none focus:border-primary/50 transition-colors resize-none"
						rows="3"
						placeholder={$t('settingsProfile.goalsPlaceholder')}
						bind:value={goals}
					></textarea>
				</div>

				<div>
					<label class="text-base font-medium text-base-content/80" for="context">
						{$t('settingsProfile.contextLabel')}
						<span class="font-normal text-base-content/90 ml-1">{$t('common.optional')}</span>
					</label>
					<textarea
						id="context"
						class="w-full mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 py-3 text-base focus:outline-none focus:border-primary/50 transition-colors resize-none"
						rows="3"
						placeholder={$t('settingsProfile.contextPlaceholder')}
						bind:value={context}
					></textarea>
				</div>
			</div>
		</section>

		<!-- Communication Style -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProfile.communicationStyle')}</h3>
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
						<div class="font-medium text-base">{$t(style.labelKey)}</div>
						<div class="text-base text-base-content/80 mt-0.5">{$t(style.descriptionKey)}</div>
					</label>
				{/each}
			</div>
		</section>

		<!-- Save -->
		{#if saveMessage}
			<Alert type={saveError ? 'error' : 'success'} title={saveError ? $t('common.error') : $t('common.saved')}>
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
					{$t('common.saving')}
				{:else}
					{$t('settingsProfile.saveProfile')}
				{/if}
			</button>
		</div>
	</form>
{/if}
