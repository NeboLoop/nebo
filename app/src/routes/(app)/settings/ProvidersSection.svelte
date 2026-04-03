<script lang="ts">
	import { onMount } from 'svelte';
	import { Key, Plus, Trash2, CheckCircle, XCircle, RefreshCw, Terminal, Wifi, Zap, ExternalLink, X } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import webapi from '$lib/api/gocliRequest';
	import type * as components from '$lib/api/neboComponents';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { t } from 'svelte-i18n';

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

	// Local models status
	let localModelsStatus = $state<components.LocalModelsStatusResponse | null>(null);

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

	const isLocalProvider = $derived(newProvider.provider === 'ollama');

	const localProviderTypes = new Set(['ollama']);

	let allProviders = $derived(() => {
		const result: {
			type: string;
			label: string;
			configured: boolean;
			isLocal: boolean;
			profile: components.AuthProfile | null;
			models: components.ModelInfo[];
		}[] = [];

		const modelProviderTypes = Object.keys(models);
		const allTypes = new Set([...modelProviderTypes, ...providerOptions.map(p => p.value)]);
		const cliProviderIds = cliProviders.map(p => p.id);

		for (const providerType of allTypes) {
			if (cliProviderIds.includes(providerType)) continue;
			if (providerType === 'janus') continue;

			const label = providerOptions.find(p => p.value === providerType)?.label || providerType;
			const profile = providers.find(p => p.provider === providerType) || null;
			const providerModels = models[providerType] || [];
			const isLocal = localProviderTypes.has(providerType);
			// Local providers are "configured" only when the runtime is actually available
			const configured = isLocal
				? (!!localModelsStatus?.available && providerModels.length > 0)
				: !!profile;

			result.push({
				type: providerType,
				label,
				configured,
				isLocal,
				profile,
				models: providerModels
			});
		}

		return result.sort((a, b) => {
			if (a.configured !== b.configured) return a.configured ? -1 : 1;
			return a.label.localeCompare(b.label);
		});
	});

	let localProvs = $derived(allProviders().filter(p => p.isLocal));
	let apiProvs = $derived(allProviders().filter(p => !p.isLocal));

	// Hide embedding models — they're always on when Nebo AI is enabled
	let janusModels = $derived(() => {
		const all = models['janus'] || [];
		return all.filter(m => !/embeddings?/i.test(m.displayName || m.id));
	});

	// Friendly display name for Janus models
	function janusDisplayName(model: components.ModelInfo): string {
		const name = model.displayName || model.id;
		if (/^janus\s*embeddings?/i.test(name)) return 'Embeddings';
		if (/^janus$/i.test(name)) return 'Nebo AI';
		return name.replace(/^janus\s*/i, 'Nebo AI ');
	}

	onMount(async () => {
		// Local models status syncs discovered Ollama models into the DB,
		// so it must complete before loadModels() reads the catalog.
		await Promise.all([loadProviders(), loadLocalModelsStatus(), loadJanusStatus()]);
		await loadModels();
		const h = () => { loadJanusStatus(); };
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

	async function loadLocalModelsStatus() {
		try {
			localModelsStatus = await api.localModelsStatus();
		} catch {
			localModelsStatus = null;
		}
	}

	async function toggleJanus(enabled: boolean) {
		if (!janusStatus?.profileId) return;
		isTogglingJanus = true;
		try {
			await api.updateAuthProfile({ metadata: { janus_provider: enabled ? 'true' : 'false' } }, janusStatus.profileId);
			await loadJanusStatus();
		} catch (err: any) {
			error = err?.message || $t('settingsProviders.toggleFailed');
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
			error = err?.message || $t('settingsProviders.loadFailed');
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
			error = err?.message || $t('settingsProviders.toggleFailed');
		}
	}

	async function toggleModel(providerType: string, model: components.ModelInfo) {
		const newActive = !model.isActive;
		model.isActive = newActive;
		try {
			// Auto-enable Janus provider when toggling a Janus model on
			if (providerType === 'janus' && newActive && janusStatus?.connected && !janusStatus.janusProvider) {
				await toggleJanus(true);
			}
			await api.updateModel({ active: newActive }, providerType, model.id);
			// NeboLoop connection stays active even when all Janus models are off.
			// The backend reload_providers() skips the Janus AI provider when no
			// models are active, but the NeboLoop comms/marketplace keep working.
		} catch (err: any) {
			model.isActive = !newActive;
			error = err?.message || $t('settingsProviders.updateFailed');
		}
	}

	async function toggleCLI(cli: components.CLIProviderInfo) {
		const newActive = !cli.active;
		cli.active = newActive;
		try {
			await api.updateCLIProvider({ active: newActive }, cli.id);
		} catch (err: any) {
			cli.active = !newActive;
			error = err?.message || $t('settingsProviders.cliUpdateFailed');
		}
	}

	async function deleteProvider(id: string) {
		if (!confirm($t('settingsProviders.deleteConfirm'))) return;
		try {
			await api.deleteAuthProfile(id);
			await loadProviders();
		} catch (err: any) {
			error = err?.message || $t('settingsProviders.deleteFailed');
		}
	}

	async function addProvider() {
		if (!newProvider.name) {
			addError = $t('settingsProviders.nameRequired');
			return;
		}
		if (!isLocalProvider && !newProvider.apiKey) {
			addError = $t('settingsProviders.apiKeyRequired');
			return;
		}

		isAdding = true;
		addError = '';
		try {
			await api.createAuthProfile({
				name: newProvider.name,
				provider: newProvider.provider,
				apiKey: newProvider.apiKey || '',
				baseUrl: newProvider.baseUrl || undefined
			});
			await loadProviders();
			await loadModels();
			closeAddModal();
		} catch (err: any) {
			addError = err?.message || $t('settingsProviders.addFailed');
		} finally {
			isAdding = false;
		}
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsProviders.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsProviders.description')}</p>
</div>

{#if isLoading}
	<div class="flex items-center justify-center gap-3 py-16">
		<Spinner size={20} />
		<span class="text-base text-base-content/80">{$t('settingsProviders.loadingProviders')}</span>
	</div>
{:else}
	<div class="space-y-6">
		{#if error}
			<Alert type="error" title={$t('common.error')}>{error}</Alert>
		{/if}

		<!-- NeboLoop AI — Primary Provider -->
		<section>
			<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProviders.neboloopAI')}</h3>
			<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
				{#if janusStatus?.connected}
					<!-- Provider header -->
					<div class="flex items-center justify-between">
						<p class="text-base font-medium text-base-content">{$t('settingsProviders.neboloopAI')}</p>
						<a href="/settings/usage" class="text-sm text-primary hover:brightness-110 transition-all">
							{$t('settingsProviders.viewUsage')}
						</a>
					</div>

					<!-- Models with toggles — same as Anthropic/DeepSeek -->
					{#if janusModels().length > 0}
						<div class="mt-3 space-y-1.5">
							{#each janusModels() as model (model.id)}
								<div class="flex items-center justify-between py-1.5 px-3 rounded-lg bg-base-content/5">
									<p class="text-base text-base-content">{janusDisplayName(model)}</p>
									<div class="flex items-center gap-3">
										<span class="text-base text-base-content/80 tabular-nums">{$t('settingsProviders.contextWindow', { values: { count: model.contextWindow?.toLocaleString() || '?' } })}</span>
										<Toggle
											checked={model.isActive}
											onchange={() => toggleModel('janus', model)}
										/>
									</div>
								</div>
							{/each}
						</div>
					{/if}

				{:else}
					<!-- Not connected -->
					<div class="flex items-center justify-between">
						<div>
							<p class="text-base font-medium text-base-content">{$t('settingsProviders.notConnected')}</p>
							<p class="text-base text-base-content/80">{$t('settingsProviders.connectDescription')}</p>
						</div>
						<a href="/settings/account" class="text-base font-medium text-primary hover:brightness-110 transition-all">
							{$t('oauth.connect')}
						</a>
					</div>
				{/if}
			</div>
		</section>

		<!-- CLI Providers -->
		{#if cliProviders.length > 0}
			<section>
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">{$t('settingsProviders.cliProviders')}</h3>
				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					<div class="space-y-2">
						{#each cliProviders as cli (cli.id)}
							<div class="flex items-center justify-between py-2.5 px-4 rounded-xl bg-base-content/5 border border-base-content/10">
								<div>
									<div class="flex items-center gap-2">
										<Terminal class="w-4 h-4 text-base-content/60" />
										<p class="text-base font-medium text-base-content">{cli.displayName}</p>
									</div>
									<p class="text-sm text-base-content/60 mt-0.5 ml-6"><code>{cli.command}</code></p>
								</div>
								<Toggle checked={cli.active} onchange={() => toggleCLI(cli)} />
							</div>
						{/each}
					</div>
				</div>
			</section>
		{/if}

		<!-- Local Models -->
		{#if localProvs.length > 0}
			<section>
				<div class="flex items-center justify-between mb-3">
					<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">{$t('settingsProviders.localModels')}</h3>
					<button
						type="button"
						class="flex items-center gap-1.5 text-sm font-medium text-base-content/60 hover:text-primary transition-colors"
						onclick={async () => { await loadLocalModelsStatus(); await loadModels(); }}
					>
						<RefreshCw class="w-3.5 h-3.5" /> {$t('settingsProviders.discover')}
					</button>
				</div>
				{#each localProvs as prov (prov.type)}
					<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
						<div class="flex items-center gap-3 mb-1">
							{#if prov.configured}
								<div class="w-2 h-2 rounded-full bg-success"></div>
							{:else}
								<div class="w-2 h-2 rounded-full bg-base-content/40"></div>
							{/if}
							<p class="text-base font-medium text-base-content">{prov.label}</p>
						</div>
						{#if prov.configured}
							<p class="text-sm text-base-content/50 ml-5 mb-3">{$t('settingsProviders.modelsDetected', { values: { count: prov.models.length } })}</p>
						{:else}
							<p class="text-sm text-base-content/50 ml-5 mb-3">{$t('settingsProviders.ollamaNotDetected')}</p>
						{/if}
						{#if prov.configured && prov.models.length > 0}
							<div class="space-y-1.5">
								{#each prov.models as model (model.id)}
									<div class="flex items-center justify-between py-1.5 px-3 rounded-lg bg-base-content/5">
										<p class="text-base text-base-content">{model.displayName}</p>
										<div class="flex items-center gap-3">
											<span class="text-sm text-base-content/60 tabular-nums">{$t('settingsProviders.contextWindow', { values: { count: model.contextWindow?.toLocaleString() || '?' } })}</span>
											<Toggle
												checked={model.isActive}
												onchange={() => toggleModel(prov.type, model)}
											/>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/each}
			</section>
		{/if}

		<!-- API Key Providers -->
		<section>
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">{$t('settingsProviders.apiKeys')}</h3>
				<button
					type="button"
					class="flex items-center gap-1.5 text-sm font-medium text-base-content/60 hover:text-primary transition-colors"
					onclick={() => openAddModal()}
				>
					<Plus class="w-4 h-4" /> {$t('settingsProviders.addProvider')}
				</button>
			</div>

			<div class="space-y-3">
				{#each apiProvs as prov (prov.type)}
					<div class="rounded-2xl bg-base-200/50 border border-base-content/10 py-3 px-4">
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-3">
								{#if prov.configured && prov.profile?.isActive}
									<div class="w-2 h-2 rounded-full bg-success"></div>
								{:else if prov.configured}
									<div class="w-2 h-2 rounded-full bg-warning"></div>
								{:else}
									<div class="w-2 h-2 rounded-full bg-base-content/40"></div>
								{/if}
								<div>
									<p class="text-base font-medium text-base-content">{prov.profile?.name || prov.label}</p>
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
									<button
										type="button"
										class="text-sm text-base-content/60 hover:text-primary transition-colors"
										onclick={() => testProvider(prov.profile!.id)}
										disabled={testingId === prov.profile.id}
									>
										{#if testingId === prov.profile.id}<Spinner size={14} />{:else}{$t('settingsProviders.test')}{/if}
									</button>
									<Toggle checked={prov.profile.isActive} onchange={() => toggleProvider(prov.profile!)} />
									<button
										type="button"
										class="text-base-content/40 hover:text-error transition-colors"
										onclick={() => deleteProvider(prov.profile!.id)}
									>
										<Trash2 class="w-4 h-4" />
									</button>
								{:else}
									<button
										type="button"
										class="text-sm font-medium text-base-content/60 hover:text-primary transition-colors"
										onclick={() => openAddModal(prov.type)}
									>
										{$t('settingsProviders.addKey')}
									</button>
								{/if}
							</div>
						</div>

						<!-- Model toggles -->
						{#if prov.models.length > 0}
							<div class="mt-3 space-y-1.5">
								{#each prov.models as model (model.id)}
									<div class="flex items-center justify-between py-1.5 px-3 rounded-lg bg-base-content/5 {!prov.configured ? 'opacity-50' : ''}">
										<p class="text-base text-base-content">{model.displayName}</p>
										<div class="flex items-center gap-3">
											<span class="text-sm text-base-content/60 tabular-nums">{$t('settingsProviders.contextWindow', { values: { count: model.contextWindow?.toLocaleString() || '?' } })}</span>
											<Toggle
												checked={prov.configured ? model.isActive : false}
												disabled={!prov.configured}
												onchange={() => toggleModel(prov.type, model)}
											/>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/each}
			</div>
		</section>
	</div>
{/if}

<!-- Add Provider Modal -->
{#if showAddForm}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="nebo-modal-backdrop" role="dialog" aria-modal="true" tabindex="-1" onkeydown={(e) => e.key === 'Escape' && closeAddModal()}>
		<button type="button" class="nebo-modal-overlay" onclick={closeAddModal}></button>
		<div class="nebo-modal-card max-w-lg">
			<!-- Header -->
			<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
				<h3 class="font-display text-lg font-bold">{$t('settingsProviders.addProviderModal')}</h3>
				<button type="button" onclick={closeAddModal} class="nebo-modal-close" aria-label={$t('common.close')}>
					<X class="w-5 h-5 text-base-content/90" />
				</button>
			</div>
			<!-- Body -->
			<div class="px-5 py-5 space-y-4">
				<div>
					<label class="text-base font-medium text-base-content/80" for="provider-type">{$t('settingsProviders.providerType')}</label>
					<select id="provider-type" bind:value={newProvider.provider} class="select w-full mt-1">
						{#each providerOptions as opt}
							<option value={opt.value}>{opt.label}</option>
						{/each}
					</select>
				</div>
				<div>
					<label class="text-base font-medium text-base-content/80" for="provider-name">{$t('settingsProviders.nameLabel')}</label>
					<input id="provider-name" type="text" bind:value={newProvider.name} placeholder={$t('settingsProviders.namePlaceholder')} class="w-full h-11 mt-1 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors" />
				</div>
				{#if !isLocalProvider}
					<div>
						<label class="text-base font-medium text-base-content/80" for="api-key">{$t('settingsProviders.apiKeyLabel')}</label>
						<input id="api-key" type="password" bind:value={newProvider.apiKey} placeholder={$t('settingsProviders.apiKeyPlaceholder')} class="w-full h-11 mt-1 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors" />
					</div>
				{/if}
				{#if isLocalProvider}
					<div>
						<label class="text-base font-medium text-base-content/80" for="base-url">{$t('settingsProviders.baseUrlLabel')} <span class="font-normal text-base-content/50">{$t('common.optional')}</span></label>
						<input id="base-url" type="text" bind:value={newProvider.baseUrl} placeholder={$t('settingsProviders.baseUrlPlaceholder')} class="w-full h-11 mt-1 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors" />
						<p class="text-sm text-base-content/50 mt-1">{$t('settingsProviders.ollamaHint')}</p>
					</div>
				{/if}
				{#if addError}
					<Alert type="error" title={$t('common.error')}>{addError}</Alert>
				{/if}
			</div>
			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
				<button
					type="button"
					class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
					onclick={closeAddModal}
				>
					{$t('common.cancel')}
				</button>
				<button
					type="button"
					class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
					onclick={addProvider}
					disabled={isAdding}
				>
					{#if isAdding}<Spinner size={16} /> {$t('settingsProviders.adding')}{:else}{$t('settingsProviders.addProviderModal')}{/if}
				</button>
			</div>
		</div>
	</div>
{/if}
