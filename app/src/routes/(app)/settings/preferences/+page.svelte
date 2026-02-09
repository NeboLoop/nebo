<script lang="ts">
	import { onMount } from 'svelte';
	import { Sun, Moon, Monitor } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import Card from '$lib/components/ui/Card.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	type Theme = 'light' | 'dark' | 'system';
	let theme = $state<Theme>('dark');
	let isLoading = $state(true);
	let saveError = $state('');

	onMount(async () => {
		try {
			const response = await api.getPreferences();
			const prefs = response.preferences;
			theme = (prefs.theme as Theme) || 'dark';
		} catch (err) {
			console.error('Failed to load preferences:', err);
		} finally {
			isLoading = false;
		}
	});

	const themeOptions = [
		{ id: 'light', label: 'Light', icon: Sun },
		{ id: 'dark', label: 'Dark', icon: Moon },
		{ id: 'system', label: 'System', icon: Monitor }
	] as const;

	async function setTheme(newTheme: Theme) {
		theme = newTheme;
		saveError = '';

		// Apply theme immediately
		if (typeof document !== 'undefined') {
			if (newTheme === 'system') {
				const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
				document.documentElement.classList.toggle('dark', prefersDark);
			} else {
				document.documentElement.classList.toggle('dark', newTheme === 'dark');
			}
		}

		// Auto-save
		try {
			await api.updatePreferences({
				theme: newTheme,
				emailNotifications: false,
				marketingEmails: false
			});
		} catch (err: any) {
			saveError = err?.message || 'Failed to save preferences';
			setTimeout(() => { saveError = ''; }, 4000);
		}
	}
</script>

{#if isLoading}
	<Card>
		<div class="flex items-center justify-center gap-3 py-8">
			<Spinner size={20} />
			<span class="text-sm text-base-content/60">Loading preferences...</span>
		</div>
	</Card>
{:else}
	<Card>
		<h3 class="text-sm font-semibold text-base-content/50 uppercase tracking-wider mb-3">Appearance</h3>

		<div>
			<span class="block text-sm font-medium text-base-content mb-2">Theme</span>
			<div class="flex gap-2" role="group">
				{#each themeOptions as option}
					<button
						onclick={() => setTheme(option.id)}
						class="flex-1 flex items-center justify-center gap-2 px-3 py-2 rounded-lg border transition-colors
							{theme === option.id
								? 'bg-primary/10 border-primary/30 text-primary'
								: 'bg-base-200 border-transparent text-base-content/60 hover:border-base-content/15'}"
					>
						<option.icon class="w-4 h-4" />
						<span class="text-sm font-medium">{option.label}</span>
					</button>
				{/each}
			</div>
		</div>
	</Card>

	{#if saveError}
		<div class="mt-4">
			<Alert type="error" title="Error">{saveError}</Alert>
		</div>
	{/if}
{/if}
