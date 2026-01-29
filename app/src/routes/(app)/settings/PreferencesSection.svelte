<script lang="ts">
	import { onMount } from 'svelte';
	import { Bell, Sun, Moon, Monitor } from 'lucide-svelte';
	import * as api from '$lib/api/gobot';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	// Theme state
	type Theme = 'light' | 'dark' | 'system';
	let theme = $state<Theme>('dark');
	let isLoading = $state(true);
	let isSaving = $state(false);
	let saveSuccess = $state(false);
	let saveError = $state('');

	// Notification preferences
	let emailNotifications = $state(true);
	let marketingEmails = $state(false);

	// Original values for change detection
	let originalPrefs = $state({
		theme: 'dark' as Theme,
		emailNotifications: true,
		marketingEmails: false
	});

	// Load preferences on mount
	onMount(async () => {
		try {
			const response = await api.getPreferences();
			const prefs = response.preferences;
			theme = (prefs.theme as Theme) || 'dark';
			emailNotifications = prefs.emailNotifications ?? true;
			marketingEmails = prefs.marketingEmails ?? false;

			originalPrefs = {
				theme,
				emailNotifications,
				marketingEmails
			};
		} catch (err) {
			console.error('Failed to load preferences:', err);
		} finally {
			isLoading = false;
		}
	});

	// Theme options
	const themeOptions = [
		{ id: 'light', label: 'Light', icon: Sun },
		{ id: 'dark', label: 'Dark', icon: Moon },
		{ id: 'system', label: 'System', icon: Monitor }
	] as const;

	function setTheme(newTheme: Theme) {
		theme = newTheme;
		saveSuccess = false;
		saveError = '';

		// Apply theme immediately for visual feedback
		if (typeof document !== 'undefined') {
			if (newTheme === 'system') {
				const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
				document.documentElement.classList.toggle('dark', prefersDark);
			} else {
				document.documentElement.classList.toggle('dark', newTheme === 'dark');
			}
		}
	}

	async function handleSave() {
		isSaving = true;
		saveSuccess = false;
		saveError = '';

		try {
			await api.updatePreferences({
				theme,
				emailNotifications,
				marketingEmails
			});
			saveSuccess = true;
			originalPrefs = {
				theme,
				emailNotifications,
				marketingEmails
			};
		} catch (err: any) {
			saveError = err?.message || 'Failed to save preferences';
		} finally {
			isSaving = false;
		}
	}

	// Track if there are unsaved changes
	const hasChanges = $derived(
		theme !== originalPrefs.theme ||
		emailNotifications !== originalPrefs.emailNotifications ||
		marketingEmails !== originalPrefs.marketingEmails
	);
</script>

<div class="space-y-6">
	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading preferences...</p>
			</div>
		</Card>
	{:else}
		<!-- Appearance -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
					<Sun class="w-5 h-5 text-primary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Appearance</h2>
					<p class="text-sm text-base-content/60">Customize how the app looks</p>
				</div>
			</div>

			<div>
				<span id="theme-label" class="block text-sm font-medium text-base-content/70 mb-3">Theme</span>
				<div class="flex gap-2" role="group" aria-labelledby="theme-label">
					{#each themeOptions as option}
						<button
							onclick={() => setTheme(option.id)}
							class="flex-1 flex items-center justify-center gap-2 px-4 py-3 rounded-lg border transition-colors
								{theme === option.id
									? 'bg-primary/10 border-primary/30 text-primary'
									: 'bg-base-200 border-base-content/20 text-base-content/70 hover:border-base-content/30'}"
						>
							<option.icon class="w-5 h-5" />
							<span class="font-medium">{option.label}</span>
						</button>
					{/each}
				</div>
			</div>
		</Card>

		<!-- Notifications -->
		<Card>
			<div class="flex items-center gap-3 mb-6">
				<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
					<Bell class="w-5 h-5 text-secondary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">Notifications</h2>
					<p class="text-sm text-base-content/60">Manage your email preferences</p>
				</div>
			</div>

			<div class="space-y-4">
				<div class="flex items-center justify-between py-3 border-b border-base-content/20">
					<div>
						<p class="text-sm font-medium text-base-content">Email Notifications</p>
						<p class="text-xs text-base-content/60">Receive important account and product notifications</p>
					</div>
					<Toggle bind:checked={emailNotifications} onchange={() => { saveSuccess = false; saveError = ''; }} />
				</div>

				<div class="flex items-center justify-between py-3">
					<div>
						<p class="text-sm font-medium text-base-content">Marketing Emails</p>
						<p class="text-xs text-base-content/60">Receive tips, offers, and promotional content</p>
					</div>
					<Toggle bind:checked={marketingEmails} onchange={() => { saveSuccess = false; saveError = ''; }} />
				</div>
			</div>
		</Card>

		<!-- Save Button -->
		{#if saveSuccess}
			<Alert type="success" title="Saved">Your preferences have been updated.</Alert>
		{/if}

		{#if saveError}
			<Alert type="error" title="Error">{saveError}</Alert>
		{/if}

		<div class="flex justify-end">
			<Button type="primary" onclick={handleSave} disabled={isSaving || !hasChanges}>
				{#if isSaving}
					<Spinner size={16} />
					Saving...
				{:else}
					Save Preferences
				{/if}
			</Button>
		</div>
	{/if}
</div>
