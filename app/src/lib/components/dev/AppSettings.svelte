<!--
  AppSettings — Configure app settings declared in manifest.json.
  Renders form fields from the settingsManifest and saves via plugin settings API.
-->

<script lang="ts">
	import { Settings, Loader2, AlertCircle } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';

	interface Props {
		appId: string;
		appName: string;
	}

	let { appId, appName }: Props = $props();

	interface SettingsOption {
		label: string;
		value: string;
	}

	interface SettingsField {
		key: string;
		title: string;
		description?: string;
		type: string;
		default?: string;
		required?: boolean;
		options?: SettingsOption[];
		placeholder?: string;
		secret?: boolean;
	}

	interface SettingsGroup {
		title: string;
		description?: string;
		fields: SettingsField[];
	}

	interface SettingsManifest {
		groups: SettingsGroup[];
	}

	let plugin = $state<components.PluginItem | null>(null);
	let manifest = $state<SettingsManifest | null>(null);
	let formValues = $state<Record<string, string>>({});
	let loading = $state(false);
	let saving = $state(false);
	let dirty = $state(false);
	let saveMessage = $state<{ text: string; type: 'success' | 'error' } | null>(null);
	let loadError = $state('');

	const allFields = $derived(manifest?.groups?.flatMap((g) => g.fields) ?? []);

	const hasRequiredEmpty = $derived(
		allFields.some((f) => f.required && !formValues[f.key]?.trim())
	);

	// Load when appName changes
	$effect(() => {
		if (appName) {
			loadSettings();
		}
	});

	async function loadSettings() {
		loading = true;
		loadError = '';
		saveMessage = null;
		dirty = false;
		try {
			const res = await api.listPlugins();
			const match = res.plugins?.find((p) => p.name === appName);
			if (!match) {
				plugin = null;
				manifest = null;
				formValues = {};
				loadError = `App "${appName}" not found in plugin registry`;
				return;
			}
			plugin = match;

			// Parse settingsManifest
			const raw = match.settingsManifest;
			if (raw && typeof raw === 'object' && Array.isArray(raw.groups)) {
				manifest = raw as SettingsManifest;
			} else {
				manifest = null;
			}

			// Initialize form values from current settings, falling back to defaults
			formValues = {};
			if (manifest?.groups) {
				for (const group of manifest.groups) {
					for (const field of group.fields) {
						formValues[field.key] = match.settings?.[field.key] ?? field.default ?? '';
					}
				}
			}
		} catch (err: any) {
			plugin = null;
			manifest = null;
			loadError = err.message || 'Failed to load settings';
		} finally {
			loading = false;
		}
	}

	function handleFieldChange(key: string, value: string) {
		formValues = { ...formValues, [key]: value };
		dirty = true;
		saveMessage = null;
	}

	async function saveSettings() {
		if (!plugin) return;
		saving = true;
		saveMessage = null;
		try {
			const secrets: Record<string, boolean> = {};
			for (const field of allFields) {
				if (field.secret) {
					secrets[field.key] = true;
				}
			}

			await api.updatePluginSettings({ settings: formValues, secrets }, plugin.id);
			saveMessage = { text: 'Settings saved', type: 'success' };
			dirty = false;

			setTimeout(() => {
				if (saveMessage?.type === 'success') saveMessage = null;
			}, 3000);
		} catch (err: any) {
			saveMessage = { text: err.message || 'Failed to save', type: 'error' };
		} finally {
			saving = false;
		}
	}
</script>

<div class="flex flex-col h-full">
	<!-- Header -->
	<div class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-100">
		<span class="text-xs text-base-content/40 flex-1">Settings</span>

		{#if saveMessage}
			<span
				class="text-xs {saveMessage.type === 'success'
					? 'text-success'
					: 'text-error'}"
			>
				{saveMessage.text}
			</span>
		{/if}
	</div>

	<!-- Content -->
	{#if !appId}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Settings class="w-8 h-8" />
			<p class="text-sm font-medium">No App Selected</p>
			<p class="text-xs">Select a project to configure</p>
		</div>
	{:else if loading}
		<div class="flex flex-col items-center justify-center flex-1 gap-2">
			<Loader2 class="w-6 h-6 text-base-content/40 animate-spin" />
			<p class="text-xs text-base-content/50">Loading settings...</p>
		</div>
	{:else if loadError}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<AlertCircle class="w-8 h-8 text-error/60" />
			<p class="text-sm text-error/80">{loadError}</p>
			<button type="button" class="btn btn-xs btn-ghost" onclick={loadSettings}>Retry</button>
		</div>
	{:else if !manifest || !manifest.groups || manifest.groups.length === 0}
		<div class="flex flex-col items-center justify-center flex-1 text-base-content/50 gap-2">
			<Settings class="w-8 h-8" />
			<p class="text-sm font-medium">No Settings Declared</p>
			<p class="text-xs">This app has no configurable settings in its manifest</p>
		</div>
	{:else}
		<div class="flex-1 min-h-0 overflow-y-auto">
			<form
				onsubmit={(e) => {
					e.preventDefault();
					saveSettings();
				}}
				class="p-4 space-y-6 max-w-lg"
			>
				{#each manifest.groups as group}
					<div class="space-y-4">
						{#if group.title}
							<div>
								<h3 class="text-sm font-semibold text-base-content">{group.title}</h3>
								{#if group.description}
									<p class="text-xs text-base-content/50 mt-0.5">
										{group.description}
									</p>
								{/if}
							</div>
						{/if}

						{#each group.fields as field (field.key)}
							<div class="form-control w-full">
								<label class="label py-1" for="field-{field.key}">
									<span class="label-text text-sm">
										{field.title}
										{#if field.required}
											<span class="text-error">*</span>
										{/if}
									</span>
								</label>

								{#if field.type === 'toggle'}
									<input
										id="field-{field.key}"
										type="checkbox"
										class="toggle toggle-primary toggle-sm"
										checked={formValues[field.key] === 'true'}
										onchange={(e) =>
											handleFieldChange(
												field.key,
												String(e.currentTarget.checked)
											)}
									/>
								{:else if field.type === 'select'}
									<select
										id="field-{field.key}"
										class="select select-bordered select-sm w-full"
										value={formValues[field.key] ?? ''}
										onchange={(e) =>
											handleFieldChange(field.key, e.currentTarget.value)}
									>
										<option value="">Select...</option>
										{#each field.options ?? [] as opt}
											<option value={opt.value}>{opt.label}</option>
										{/each}
									</select>
								{:else if field.type === 'number'}
									<input
										id="field-{field.key}"
										type="number"
										class="input input-bordered input-sm w-full"
										value={formValues[field.key] ?? ''}
										placeholder={field.placeholder ?? ''}
										oninput={(e) =>
											handleFieldChange(field.key, e.currentTarget.value)}
									/>
								{:else if field.type === 'password'}
									<input
										id="field-{field.key}"
										type="password"
										class="input input-bordered input-sm w-full"
										value={formValues[field.key] ?? ''}
										placeholder={field.placeholder ?? ''}
										oninput={(e) =>
											handleFieldChange(field.key, e.currentTarget.value)}
									/>
								{:else if field.type === 'url'}
									<input
										id="field-{field.key}"
										type="url"
										class="input input-bordered input-sm w-full"
										value={formValues[field.key] ?? ''}
										placeholder={field.placeholder ?? 'https://...'}
										oninput={(e) =>
											handleFieldChange(field.key, e.currentTarget.value)}
									/>
								{:else}
									<input
										id="field-{field.key}"
										type="text"
										class="input input-bordered input-sm w-full"
										value={formValues[field.key] ?? ''}
										placeholder={field.placeholder ?? ''}
										oninput={(e) =>
											handleFieldChange(field.key, e.currentTarget.value)}
									/>
								{/if}

								{#if field.description}
									<label class="label py-0.5" for="field-{field.key}">
										<span class="label-text-alt text-base-content/40"
											>{field.description}</span
										>
									</label>
								{/if}
							</div>
						{/each}
					</div>
				{/each}

				<div class="pt-2">
					<button
						type="submit"
						class="btn btn-sm btn-primary"
						disabled={saving || !dirty || hasRequiredEmpty}
					>
						{#if saving}
							<Loader2 class="w-4 h-4 animate-spin" />
							Saving...
						{:else}
							Save Settings
						{/if}
					</button>
				</div>
			</form>
		</div>
	{/if}
</div>
