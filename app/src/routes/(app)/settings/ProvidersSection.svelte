<script lang="ts">
	import { onMount } from 'svelte';
	import { Key, Plus, Trash2, CheckCircle, XCircle, RefreshCw, Terminal, Wifi, Zap, ExternalLink } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import webapi from '$lib/api/gocliRequest';
	import type * as components from '$lib/api/neboComponents';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let providers = $state<components.AuthProfile[]>([]);
	let models = $state<{ [key: string]: components.ModelInfo[] }>({});
	let availableCLIs = $state<components.CLIAvailability | null>(null);
	let error = $state('');
	let testingId = $state<string | null>(null);
	let testResult = $state<{ id: string; success: boolean; message: string } | null>(null);
	let isTogglingJanus = $state(false);

	// Janus / NeboLoop status
	let janusStatus = $state<components.NeboLoopAccountStatusResponse | null>(null);
	let janusUsage = $state<components.NeboLoopJanusUsageResponse | null>(null);

	// New provider form
	let showAddForm = $state(false);
	let newProvider = $state({
		name: '',
		provider: 'anthropic',
		apiKey: '',
		baseUrl: ''
	});
	let isAdding = $state(false);
	let addError = $state('');

	// CLI providers from API
	let cliProviders = $state<components.CLIProviderInfo[]>([]);

	function openAddModal(providerType?: string) {
		if (providerType) {
			const label = providerOptions.find(p => p.value === providerType)?.label || providerType;
			newProvider = { name: `My ${label}`, provider: providerType, apiKey: '', baseUrl: '' };
		} else {
			newProvider = { name: '', provider: 'anthropic', apiKey: '', baseUrl: '' };
		}
		addError = '';
		showAddForm = true;
	}

	function closeAddModal() {
		showAddForm = false;
		newProvider = { name: '', provider: 'anthropic', apiKey: '', baseUrl: '' };
		addError = '';
	}

	const providerOptions = [
		{ value: 'anthropic', label: 'Anthropic (Claude)' },
		{ value: 'openai', label: 'OpenAI (GPT)' },
		{ value: 'google', label: 'Google (Gemini)' },
		{ value: 'deepseek', label: 'DeepSeek' },
		{ value: 'ollama', label: 'Ollama (Local)' }
	];

	// Merge models.yaml catalog with auth_profiles to show all providers
	// Excludes CLI providers (shown separately) and Janus (shown separately)
	let allProviders = $derived(() => {
		const result: {
			type: string;
			label: string;
			configured: boolean;
			profile: components.AuthProfile | null;
			models: components.ModelInfo[];
		}[] = [];

		const modelProviderTypes = Object.keys(models);
		const allTypes = new Set([...modelProviderTypes, ...providerOptions.map(p => p.value)]);

		// Skip CLI providers and Janus (they're shown separately)
		const cliProviderIds = cliProviders.map(p => p.id);

		for (const providerType of allTypes) {
			if (cliProviderIds.includes(providerType)) continue;
			if (providerType === 'janus') continue;

			const label = providerOptions.find(p => p.value === providerType)?.label || providerType;
			const profile = providers.find(p => p.provider === providerType) || null;
			const providerModels = models[providerType] || [];

			result.push({
				type: providerType,
				label,
				configured: !!profile,
				profile,
				models: providerModels
			});
		}

		return result.sort((a, b) => {
			if (a.configured !== b.configured) return a.configured ? -1 : 1;
			return a.label.localeCompare(b.label);
		});
	});

	// Janus models (shown in their own section)
	let janusModels = $derived(() => models['janus'] || []);

	onMount(async () => {
		await Promise.all([loadProviders(), loadModels(), loadJanusStatus(), loadJanusUsage()]);
		const h = () => { loadJanusStatus(); loadJanusUsage(); };
		window.addEventListener('nebo:plan_changed', h);
		return () => window.removeEventListener('nebo:plan_changed', h);
	});

	async function loadJanusStatus() {
		try {
			janusStatus = await api.neboLoopAccountStatus();
		} catch {
			janusStatus = null;
		}
	}

	async function loadJanusUsage() {
		try {
			janusUsage = await api.neboLoopJanusUsage();
		} catch {
			janusUsage = null;
		}
	}

	async function toggleJanus(enabled: boolean) {
		if (!janusStatus?.profileId) return;
		isTogglingJanus = true;
		try {
			await api.updateAuthProfile({ metadata: { janus_provider: enabled ? 'true' : 'false' } }, janusStatus.profileId);
			await loadJanusStatus();
		} catch (err: any) {
			error = err?.message || 'Failed to toggle Janus';
		} finally {
			isTogglingJanus = false;
		}
	}

	async function loadProviders() {
		isLoading = true;
		error = '';
		try {
			const response = await api.listAuthProfiles();
			providers = response.profiles || [];
		} catch (err: any) {
			error = err?.message || 'Failed to load providers';
		} finally {
			isLoading = false;
		}
	}

	async function loadModels() {
		try {
			const response = await api.listModels();
			models = response.models || {};
			availableCLIs = response.availableCLIs || null;

			cliProviders = response.cliProviders || [];
		} catch (err: any) {
			console.error('Failed to load models:', err);
		}
	}

	async function testProvider(id: string) {
		testingId = id;
		testResult = null;
		try {
			const response = await api.testAuthProfile(id);
			testResult = { id, success: response.success, message: response.message };
		} catch (err: any) {
			testResult = { id, success: false, message: err?.message || 'Test failed' };
		} finally {
			testingId = null;
		}
	}

	async function toggleProvider(provider: components.AuthProfile) {
		const newActive = !provider.isActive;
		provider.isActive = newActive;
		try {
			await api.updateAuthProfile({ isActive: newActive }, provider.id);
		} catch (err: any) {
			provider.isActive = !newActive;
			error = err?.message || 'Failed to update provider';
		}
	}

	async function toggleModel(providerType: string, model: components.ModelInfo) {
		const newActive = !model.isActive;
		model.isActive = newActive;
		try {
			await api.updateModel({ active: newActive }, providerType, model.id);
		} catch (err: any) {
			model.isActive = !newActive;
			error = err?.message || 'Failed to update model';
		}
	}

	async function toggleCLI(cli: components.CLIProviderInfo) {
		const newActive = !cli.active;
		cli.active = newActive;
		try {
			await api.updateCLIProvider({ active: newActive }, cli.id);
		} catch (err: any) {
			cli.active = !newActive;
			error = err?.message || 'Failed to update CLI provider';
		}
	}

	async function deleteProvider(id: string) {
		if (!confirm('Are you sure you want to delete this provider?')) return;
		try {
			await api.deleteAuthProfile(id);
			await loadProviders();
		} catch (err: any) {
			error = err?.message || 'Failed to delete provider';
		}
	}

	async function addProvider() {
		if (!newProvider.name || (!newProvider.apiKey && newProvider.provider !== 'ollama')) {
			addError = 'Name and API key are required';
			return;
		}

		isAdding = true;
		addError = '';
		try {
			await api.createAuthProfile({
				name: newProvider.name,
				provider: newProvider.provider,
				apiKey: newProvider.apiKey,
				baseUrl: newProvider.baseUrl || undefined
			});
			await loadProviders();
			closeAddModal();
		} catch (err: any) {
			addError = err?.message || 'Failed to add provider';
		} finally {
			isAdding = false;
		}
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">Providers</h2>
	<p class="text-sm text-base-content/60">AI model providers and API keys</p>
</div>

<div class="space-y-6">
	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading providers...</p>
			</div>
		</Card>
	{:else}
		{#if error}
			<Alert type="error" title="Error">{error}</Alert>
		{/if}

		<!-- Connection Status Banner -->
		{#if janusStatus?.connected && janusStatus.janusProvider}
			<div class="flex items-center gap-3 rounded-lg bg-success/10 px-4 py-3">
				<Wifi class="w-5 h-5 text-success" />
				<div class="flex-1">
					<p class="text-sm font-medium text-success">Connected via NeboLoop</p>
					{#if janusStatus.email}
						<p class="text-xs text-base-content/60">{janusStatus.email}</p>
					{/if}
				</div>
				<Toggle checked={true} disabled={isTogglingJanus} onchange={() => toggleJanus(false)} />
			</div>
		{:else if janusStatus?.connected && !janusStatus.janusProvider}
			<div class="flex items-center gap-3 rounded-lg bg-base-200/50 px-4 py-3">
				<Wifi class="w-5 h-5 text-base-content/40" />
				<div class="flex-1">
					<p class="text-sm font-medium text-base-content">NeboLoop Connected</p>
					<p class="text-xs text-base-content/60">Enable Janus AI to use NeboLoop as your AI provider</p>
				</div>
				<Toggle checked={false} disabled={isTogglingJanus} onchange={() => toggleJanus(true)} />
			</div>
		{:else if !janusStatus?.connected && providers.length === 0 && !availableCLIs?.claude && !availableCLIs?.codex && !availableCLIs?.gemini}
			<div class="flex items-center gap-3 rounded-lg bg-warning/10 px-4 py-3">
				<XCircle class="w-5 h-5 text-warning" />
				<p class="text-sm font-medium text-warning">No AI providers configured</p>
			</div>
		{/if}

		<!-- Janus (NeboLoop AI Gateway) -->
		{#if janusModels().length > 0}
			<Card>
				<div class="flex items-center gap-3 mb-3">
					<Zap class="w-5 h-5 text-primary" />
					<div class="flex-1">
						<h4 class="font-medium text-base-content">Janus</h4>
						<p class="text-xs text-base-content/60">NeboLoop AI Gateway — {janusModels().filter(m => m.isActive).length} active model{janusModels().filter(m => m.isActive).length !== 1 ? 's' : ''}</p>
					</div>
					<button class="btn btn-ghost btn-xs gap-1 text-base-content/50" onclick={() => webapi.get('/api/v1/neboloop/open', { path: '/app/settings/billing' })}>
						Upgrade
						<ExternalLink class="w-3 h-3" />
					</button>
				</div>
			{#if janusUsage && (janusUsage.session.limitTokens > 0 || janusUsage.weekly.limitTokens > 0)}
				<div class="flex flex-col gap-2 px-4 py-2 mb-3">
					{#if janusUsage.session.limitTokens > 0}
						<div>
							<div class="flex justify-between text-xs text-base-content/60 mb-1">
								<span>Session: {janusUsage.session.percentUsed}% used</span>
								{#if janusUsage.session.resetAt}
									{@const reset = new Date(janusUsage.session.resetAt)}
									{@const now = new Date()}
									{@const diffMs = reset.getTime() - now.getTime()}
									{@const diffH = Math.floor(diffMs / 3600000)}
									{@const diffM = Math.floor((diffMs % 3600000) / 60000)}
									<span>Resets in {diffH}h {diffM}m</span>
								{/if}
							</div>
							<progress
								class="progress w-full {janusUsage.session.percentUsed > 80 ? 'progress-warning' : 'progress-primary'}"
								value={janusUsage.session.percentUsed}
								max="100"
							></progress>
						</div>
					{/if}
					{#if janusUsage.weekly.limitTokens > 0}
						<div>
							<div class="flex justify-between text-xs text-base-content/60 mb-1">
								<span>Weekly: {janusUsage.weekly.percentUsed}% used</span>
								{#if janusUsage.weekly.resetAt}
									<span>Resets {new Date(janusUsage.weekly.resetAt).toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric' })}</span>
								{/if}
							</div>
							<progress
								class="progress w-full {janusUsage.weekly.percentUsed > 80 ? 'progress-warning' : 'progress-primary'}"
								value={janusUsage.weekly.percentUsed}
								max="100"
							></progress>
						</div>
					{/if}
				</div>
			{/if}
				<div class="grid gap-2">
					{#each janusModels() as model (model.id)}
						<div class="flex items-center justify-between py-2 px-3 rounded-lg bg-base-200/30">
							<div class="flex-1">
								<p class="font-medium text-sm text-base-content">{model.displayName}</p>
							</div>
							<div class="flex items-center gap-3">
								<p class="text-xs text-base-content/50 tabular-nums">{model.contextWindow?.toLocaleString() || '?'} tokens</p>
								<Toggle
									checked={model.isActive}
									onchange={() => toggleModel('janus', model)}
								/>
							</div>
						</div>
					{/each}
				</div>
			</Card>
		{/if}

		<!-- CLI Providers -->
		{#if cliProviders.length > 0}
			<Card>
				<div class="flex items-center gap-3 mb-3">
					<Terminal class="w-5 h-5 text-accent" />
					<div>
						<h4 class="font-medium text-base-content">CLI Providers</h4>
						<p class="text-xs text-base-content/60">Locally installed AI tools — no API key needed</p>
					</div>
				</div>
				<div class="grid gap-2">
					{#each cliProviders as cli (cli.id)}
						<div class="flex items-center justify-between py-2 px-3 rounded-lg bg-base-200/30">
							<div>
								<p class="font-medium text-sm text-base-content">{cli.displayName}</p>
								<p class="text-xs text-base-content/60"><code>{cli.command}</code> — {cli.installHint}</p>
							</div>
							<Toggle checked={cli.active} onchange={() => toggleCLI(cli)} />
						</div>
					{/each}
				</div>
			</Card>
		{/if}

		<!-- API Providers -->
		<Card>
			<div class="flex items-center justify-between mb-4">
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
						<Key class="w-5 h-5 text-primary" />
					</div>
					<div>
						<h3 class="text-lg font-semibold text-base-content">API Keys</h3>
						<p class="text-sm text-base-content/60">Manage provider connections and models</p>
					</div>
				</div>
				<Button type="primary" size="sm" onclick={() => openAddModal()}>
					<Plus class="w-4 h-4" />
					Add Provider
				</Button>
			</div>

			<div class="divide-y divide-base-200">
				{#each allProviders() as prov, i (prov.type)}
					<div class="py-4 first:pt-0 last:pb-0">
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-4">
								<div class="w-10 h-10 rounded-lg flex items-center justify-center {prov.configured && prov.profile?.isActive ? 'bg-success/10' : prov.configured ? 'bg-warning/10' : 'bg-base-200'}">
									{#if prov.configured && prov.profile?.isActive}
										<CheckCircle class="w-5 h-5 text-success" />
									{:else if prov.configured}
										<XCircle class="w-5 h-5 text-warning" />
									{:else}
										<Key class="w-5 h-5 text-base-content/40" />
									{/if}
								</div>
								<div>
									<h4 class="font-medium text-base-content">{prov.profile?.name || prov.label}</h4>
									{#if prov.profile?.name && prov.profile.name !== prov.label}
										<p class="text-sm text-base-content/60">{prov.label}</p>
									{/if}
								</div>
							</div>
							<div class="flex items-center gap-3">
								{#if prov.configured && prov.profile}
									{#if testResult?.id === prov.profile.id}
										<span class="text-sm {testResult.success ? 'text-success' : 'text-error'}">{testResult.message}</span>
									{/if}
									<Button type="ghost" size="sm" onclick={() => testProvider(prov.profile!.id)} disabled={testingId === prov.profile.id}>
										{#if testingId === prov.profile.id}<Spinner size={16} />{:else}<RefreshCw class="w-4 h-4" />{/if} Test
									</Button>
									<Toggle checked={prov.profile.isActive} onchange={() => toggleProvider(prov.profile!)} />
									<Button type="ghost" size="sm" onclick={() => deleteProvider(prov.profile!.id)}>
										<Trash2 class="w-4 h-4 text-error" />
									</Button>
								{:else}
									<button type="button" class="text-xs text-base-content/40 hover:text-primary transition-colors" onclick={() => openAddModal(prov.type)}>
										Add key
									</button>
								{/if}
							</div>
						</div>
						{#if prov.models.length > 0}
							<div class="mt-3 grid gap-2">
								{#each prov.models as model (model.id)}
									<div class="flex items-center justify-between py-2 px-3 rounded-lg bg-base-200/30">
										<div class="flex-1">
											<p class="font-medium text-sm text-base-content">{model.displayName}</p>
										</div>
										<div class="flex items-center gap-3">
											<p class="text-xs text-base-content/50 tabular-nums">{model.contextWindow?.toLocaleString() || '?'} tokens</p>
											<Toggle checked={model.isActive} onchange={() => toggleModel(prov.type, model)} />
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/each}
			</div>
		</Card>

	{/if}
</div>

<!-- Add Provider Modal -->
{#if showAddForm}
	<div class="modal modal-open">
		<div class="modal-box">
			<h3 class="font-bold text-lg mb-4">Add Provider</h3>
			<div class="space-y-4">
				<div>
					<label for="provider-type" class="block text-sm font-medium text-base-content mb-1">Provider Type</label>
					<select id="provider-type" bind:value={newProvider.provider} class="select select-bordered w-full">
						{#each providerOptions as opt}
							<option value={opt.value}>{opt.label}</option>
						{/each}
					</select>
				</div>
				<div>
					<label for="provider-name" class="block text-sm font-medium text-base-content mb-1">Name</label>
					<input id="provider-name" type="text" bind:value={newProvider.name} placeholder="My Anthropic Key" class="input input-bordered w-full" />
				</div>
				<div>
					<label for="api-key" class="block text-sm font-medium text-base-content mb-1">API Key</label>
					<input id="api-key" type="password" bind:value={newProvider.apiKey} placeholder={newProvider.provider === 'ollama' ? 'Not required for Ollama' : 'sk-...'} class="input input-bordered w-full" />
				</div>
				{#if newProvider.provider === 'ollama'}
					<div>
						<label for="base-url" class="block text-sm font-medium text-base-content mb-1">Base URL (optional)</label>
						<input id="base-url" type="text" bind:value={newProvider.baseUrl} placeholder="http://localhost:11434" class="input input-bordered w-full" />
					</div>
				{/if}
				{#if addError}
					<Alert type="error" title="Error">{addError}</Alert>
				{/if}
			</div>
			<div class="modal-action">
				<Button type="ghost" onclick={closeAddModal}>Cancel</Button>
				<Button type="primary" onclick={addProvider} disabled={isAdding}>
					{#if isAdding}<Spinner size={16} /> Adding...{:else}Add Provider{/if}
				</Button>
			</div>
		</div>
		<button type="button" class="modal-backdrop" onclick={closeAddModal}>close</button>
	</div>
{/if}

