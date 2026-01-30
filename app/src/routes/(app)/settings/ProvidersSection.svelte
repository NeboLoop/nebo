<script lang="ts">
	import { onMount } from 'svelte';
	import { Key, Plus, Trash2, CheckCircle, XCircle, RefreshCw, Cpu, Eye, Code, Brain, Sparkles, Terminal, Tag, Volume2, Star } from 'lucide-svelte';
	import * as api from '$lib/api/gobot';
	import type * as components from '$lib/api/gobotComponents';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';

	let isLoading = $state(true);
	let providers = $state<components.AuthProfile[]>([]);
	let models = $state<{ [key: string]: components.ModelInfo[] }>({});
	let taskRouting = $state<components.TaskRouting | null>(null);
	let aliases = $state<components.ModelAlias[]>([]);
	let availableCLIs = $state<components.CLIAvailability | null>(null);
	let error = $state('');
	let testingId = $state<string | null>(null);
	let testResult = $state<{ id: string; success: boolean; message: string } | null>(null);

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

	// Task routing editing
	let showRoutingConfig = $state(false);
	let routingForm = $state({
		vision: '',
		audio: '',
		reasoning: '',
		code: '',
		general: ''
	});
	let isSavingRouting = $state(false);

	// Aliases editing
	let showAliasesConfig = $state(false);
	let aliasesForm = $state<{ alias: string; modelId: string }[]>([]);
	let isSavingAliases = $state(false);

	// CLI Provider modal
	let showCLIModal = $state(false);
	let selectedCLI = $state<{ id: string; name: string; command: string; installHint: string } | null>(null);

	const cliProviderInfo: { [key: string]: { id: string; name: string; command: string; installHint: string } } = {
		claude: { id: 'claude-cli', name: 'Claude Code', command: 'claude', installHint: 'brew install claude-code' },
		codex: { id: 'codex-cli', name: 'Codex CLI', command: 'codex', installHint: 'npm i -g @openai/codex' },
		gemini: { id: 'gemini-cli', name: 'Gemini CLI', command: 'gemini', installHint: 'npm i -g @google/gemini-cli' }
	};

	function openCLIModal(cliKey: string) {
		selectedCLI = cliProviderInfo[cliKey];
		showCLIModal = true;
	}

	function closeCLIModal() {
		showCLIModal = false;
		selectedCLI = null;
	}

	function getCLIModels(cliId: string): components.ModelInfo[] {
		return models[cliId] || [];
	}

	const providerOptions = [
		{ value: 'anthropic', label: 'Anthropic (Claude)' },
		{ value: 'openai', label: 'OpenAI (GPT)' },
		{ value: 'google', label: 'Google (Gemini)' },
		{ value: 'ollama', label: 'Ollama (Local)' }
	];

	onMount(async () => {
		await Promise.all([loadProviders(), loadModels()]);
	});

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
			taskRouting = response.taskRouting || null;
			aliases = response.aliases || [];
			availableCLIs = response.availableCLIs || null;
			// Initialize routing form with current values
			if (taskRouting) {
				routingForm = {
					vision: taskRouting.vision || '',
					audio: taskRouting.audio || '',
					reasoning: taskRouting.reasoning || '',
					code: taskRouting.code || '',
					general: taskRouting.general || ''
				};
			}
			// Initialize aliases form
			aliasesForm = aliases.map((a) => ({ alias: a.alias, modelId: a.modelId }));
		} catch (err: any) {
			console.error('Failed to load models:', err);
		}
	}

	async function saveTaskRouting() {
		isSavingRouting = true;
		try {
			await api.updateTaskRouting({
				vision: routingForm.vision || undefined,
				audio: routingForm.audio || undefined,
				reasoning: routingForm.reasoning || undefined,
				code: routingForm.code || undefined,
				general: routingForm.general || undefined
			});
			await loadModels();
			showRoutingConfig = false;
		} catch (err: any) {
			error = err?.message || 'Failed to save task routing';
		} finally {
			isSavingRouting = false;
		}
	}

	async function saveAliases() {
		isSavingAliases = true;
		try {
			// Filter out empty aliases
			const validAliases = aliasesForm.filter((a) => a.alias.trim() && a.modelId);
			await api.updateTaskRouting({
				aliases: validAliases
			});
			await loadModels();
			showAliasesConfig = false;
		} catch (err: any) {
			error = err?.message || 'Failed to save aliases';
		} finally {
			isSavingAliases = false;
		}
	}

	function addAlias() {
		aliasesForm = [...aliasesForm, { alias: '', modelId: '' }];
	}

	function removeAlias(index: number) {
		aliasesForm = aliasesForm.filter((_, i) => i !== index);
	}

	// Get all available model options for dropdowns
	function getAllModelOptions(): { value: string; label: string }[] {
		const options: { value: string; label: string }[] = [];
		for (const [providerType, modelList] of Object.entries(models)) {
			for (const model of modelList) {
				if (model.isActive) {
					options.push({
						value: `${providerType}/${model.id}`,
						label: `${model.displayName} (${providerType})`
					});
				}
			}
		}
		return options;
	}

	// Get capability badge color
	function getCapabilityColor(cap: string): string {
		switch (cap) {
			case 'vision': return 'badge-info';
			case 'reasoning': return 'badge-secondary';
			case 'code': return 'badge-accent';
			case 'tools': return 'badge-success';
			case 'streaming': return 'badge-ghost';
			default: return 'badge-neutral';
		}
	}

	async function testProvider(id: string) {
		testingId = id;
		testResult = null;
		try {
			const response = await api.testAuthProfile({}, id);
			testResult = { id, success: response.success, message: response.message };
		} catch (err: any) {
			testResult = { id, success: false, message: err?.message || 'Test failed' };
		} finally {
			testingId = null;
		}
	}

	async function toggleProvider(provider: components.AuthProfile) {
		try {
			await api.updateAuthProfile({}, { isActive: !provider.isActive }, provider.id);
			await loadProviders();
		} catch (err: any) {
			error = err?.message || 'Failed to update provider';
		}
	}

	async function toggleModel(providerType: string, model: components.ModelInfo) {
		try {
			await api.updateModel({}, { active: !model.isActive }, providerType, model.id);
			await loadModels();
		} catch (err: any) {
			error = err?.message || 'Failed to update model';
		}
	}

	async function togglePreferred(providerType: string, model: components.ModelInfo) {
		try {
			await api.updateModel({}, { preferred: !model.preferred }, providerType, model.id);
			await loadModels();
		} catch (err: any) {
			error = err?.message || 'Failed to update model';
		}
	}

	// Available kind options
	const kindOptions = ['fast', 'smart', 'code', 'cheap', 'reason', 'local', 'default'];

	async function toggleKind(providerType: string, model: components.ModelInfo, kind: string) {
		const currentKinds = model.kind || [];
		const newKinds = currentKinds.includes(kind)
			? currentKinds.filter((k) => k !== kind)
			: [...currentKinds, kind];
		try {
			await api.updateModel({}, { kind: newKinds }, providerType, model.id);
			await loadModels();
		} catch (err: any) {
			error = err?.message || 'Failed to update model';
		}
	}

	async function deleteProvider(id: string) {
		if (!confirm('Are you sure you want to delete this provider?')) return;
		try {
			await api.deleteAuthProfile({}, id);
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
			showAddForm = false;
			newProvider = { name: '', provider: 'anthropic', apiKey: '', baseUrl: '' };
		} catch (err: any) {
			addError = err?.message || 'Failed to add provider';
		} finally {
			isAdding = false;
		}
	}

	function getProviderLabel(providerType: string) {
		return providerOptions.find((p) => p.value === providerType)?.label || providerType;
	}

	function getProviderModels(providerType: string) {
		return models[providerType] || [];
	}
</script>

<div class="space-y-6">
	{#if isLoading}
		<Card>
			<div class="flex flex-col items-center justify-center gap-4 py-8">
				<Spinner size={32} />
				<p class="text-sm text-base-content/60">Loading providers...</p>
			</div>
		</Card>
	{:else}
		<!-- Header with Add Button -->
		<div class="flex items-center justify-between">
			<div class="flex items-center gap-3">
				<div class="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center">
					<Key class="w-5 h-5 text-primary" />
				</div>
				<div>
					<h2 class="text-lg font-semibold text-base-content">AI Providers</h2>
					<p class="text-sm text-base-content/60">Manage API keys and available models</p>
				</div>
			</div>
			<Button type="primary" onclick={() => (showAddForm = !showAddForm)}>
				<Plus class="w-4 h-4" />
				Add Provider
			</Button>
		</div>

		{#if error}
			<Alert type="error" title="Error">{error}</Alert>
		{/if}

		<!-- Add Provider Form -->
		{#if showAddForm}
			<Card>
				<h3 class="text-lg font-semibold text-base-content mb-4">Add New Provider</h3>

				<div class="space-y-4">
					<div>
						<label for="provider-type" class="block text-sm font-medium text-base-content mb-1">
							Provider Type
						</label>
						<select
							id="provider-type"
							bind:value={newProvider.provider}
							class="select select-bordered w-full"
						>
							{#each providerOptions as opt}
								<option value={opt.value}>{opt.label}</option>
							{/each}
						</select>
					</div>

					<div>
						<label for="provider-name" class="block text-sm font-medium text-base-content mb-1">
							Name
						</label>
						<input
							id="provider-name"
							type="text"
							bind:value={newProvider.name}
							placeholder="My Anthropic Key"
							class="input input-bordered w-full"
						/>
					</div>

					<div>
						<label for="api-key" class="block text-sm font-medium text-base-content mb-1">
							API Key
						</label>
						<input
							id="api-key"
							type="password"
							bind:value={newProvider.apiKey}
							placeholder={newProvider.provider === 'ollama' ? 'Not required for Ollama' : 'sk-...'}
							class="input input-bordered w-full"
						/>
					</div>

					{#if newProvider.provider === 'ollama'}
						<div>
							<label for="base-url" class="block text-sm font-medium text-base-content mb-1">
								Base URL (optional)
							</label>
							<input
								id="base-url"
								type="text"
								bind:value={newProvider.baseUrl}
								placeholder="http://localhost:11434"
								class="input input-bordered w-full"
							/>
						</div>
					{/if}

					{#if addError}
						<Alert type="error" title="Error">{addError}</Alert>
					{/if}

					<div class="flex gap-2 justify-end">
						<Button type="ghost" onclick={() => (showAddForm = false)}>Cancel</Button>
						<Button type="primary" onclick={addProvider} disabled={isAdding}>
							{#if isAdding}
								<Spinner size={16} />
								Adding...
							{:else}
								Add Provider
							{/if}
						</Button>
					</div>
				</div>
			</Card>
		{/if}

		<!-- Provider List with Inline Models -->
		{#if providers.length === 0}
			<Card>
				<div class="text-center py-8">
					<Key class="w-12 h-12 text-base-content/30 mx-auto mb-4" />
					<h3 class="text-lg font-medium text-base-content mb-2">No providers configured</h3>
					<p class="text-sm text-base-content/60 mb-4">
						Add an AI provider to start using GoBot.
					</p>
					<Button type="primary" onclick={() => (showAddForm = true)}>
						<Plus class="w-4 h-4" />
						Add Your First Provider
					</Button>
				</div>
			</Card>
		{:else}
			<div class="space-y-4">
				{#each providers as provider (provider.id)}
					{@const providerModels = getProviderModels(provider.provider)}
					<Card>
						<!-- Provider Header -->
						<div class="flex items-center justify-between">
							<div class="flex items-center gap-4">
								<div
									class="w-10 h-10 rounded-lg flex items-center justify-center {provider.isActive
										? 'bg-success/10'
										: 'bg-base-200'}"
								>
									{#if provider.isActive}
										<CheckCircle class="w-5 h-5 text-success" />
									{:else}
										<XCircle class="w-5 h-5 text-base-content/40" />
									{/if}
								</div>
								<div>
									<h4 class="font-medium text-base-content">{provider.name}</h4>
									<p class="text-sm text-base-content/60">
										{getProviderLabel(provider.provider)}
									</p>
								</div>
							</div>

							<div class="flex items-center gap-3">
								{#if testResult?.id === provider.id}
									<span class="text-sm {testResult.success ? 'text-success' : 'text-error'}">
										{testResult.message}
									</span>
								{/if}

								<Button
									type="ghost"
									size="sm"
									onclick={() => testProvider(provider.id)}
									disabled={testingId === provider.id}
								>
									{#if testingId === provider.id}
										<Spinner size={16} />
									{:else}
										<RefreshCw class="w-4 h-4" />
									{/if}
									Test
								</Button>

								<Toggle
									checked={provider.isActive}
									onchange={() => toggleProvider(provider)}
								/>

								<Button
									type="ghost"
									size="sm"
									onclick={() => deleteProvider(provider.id)}
								>
									<Trash2 class="w-4 h-4 text-error" />
								</Button>
							</div>
						</div>

						<!-- Models for this provider - always visible -->
						{#if providerModels.length > 0}
							<div class="mt-4 pt-4 border-t border-base-200">
								<div class="grid gap-2">
									{#each providerModels as model (model.id)}
										<div class="flex items-center justify-between py-2 px-3 rounded-lg bg-base-200/30">
											<div class="flex-1">
												<div class="flex items-center gap-2">
													<button
														type="button"
														class="btn btn-ghost btn-xs p-0 min-h-0 h-auto"
														onclick={() => togglePreferred(provider.provider, model)}
														title={model.preferred ? 'Remove preferred' : 'Set as preferred'}
													>
														<Star class="w-4 h-4 {model.preferred ? 'text-warning fill-warning' : 'text-base-content/30'}" />
													</button>
													<p class="font-medium text-sm text-base-content">{model.displayName}</p>
												</div>
												<div class="flex gap-2 flex-wrap mt-1">
													{#each kindOptions as kind}
														<label class="flex items-center gap-1 cursor-pointer">
															<input
																type="checkbox"
																class="checkbox checkbox-xs checkbox-primary"
																checked={(model.kind || []).includes(kind)}
																onchange={() => toggleKind(provider.provider, model, kind)}
															/>
															<span class="text-xs text-base-content/70">{kind}</span>
														</label>
													{/each}
												</div>
												<p class="text-xs text-base-content/50">
													{model.contextWindow?.toLocaleString() || '?'} tokens
												</p>
											</div>
											<Toggle
												checked={model.isActive}
												onchange={() => toggleModel(provider.provider, model)}
											/>
										</div>
									{/each}
								</div>
							</div>
						{/if}
					</Card>
				{/each}
			</div>
		{/if}

		<!-- CLI Providers Section -->
		{#if availableCLIs && (availableCLIs.claude || availableCLIs.codex || availableCLIs.gemini)}
			<Card>
				<div class="flex items-center gap-3 mb-4">
					<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center">
						<Terminal class="w-5 h-5 text-accent" />
					</div>
					<div>
						<h3 class="text-lg font-semibold text-base-content">CLI Providers</h3>
						<p class="text-sm text-base-content/60">AI coding assistants detected on your system</p>
					</div>
				</div>

				<div class="space-y-3">
					{#if availableCLIs.claude}
						{@const cliModels = getCLIModels('claude-cli')}
						<div class="border border-base-300 rounded-lg overflow-hidden">
							<button
								type="button"
								class="w-full flex items-center justify-between py-3 px-4 bg-base-200/30 hover:bg-base-200/50 transition-colors cursor-pointer"
								onclick={() => openCLIModal('claude')}
							>
								<div class="flex items-center gap-3">
									<div class="w-8 h-8 rounded-lg bg-success/10 flex items-center justify-center">
										<CheckCircle class="w-4 h-4 text-success" />
									</div>
									<div class="text-left">
										<p class="font-medium text-base-content">Claude Code</p>
										<p class="text-xs text-base-content/60">Anthropic's agentic coding assistant</p>
									</div>
								</div>
								<span class="badge badge-success badge-sm">Available</span>
							</button>
							{#if cliModels.length > 0}
								<div class="divide-y divide-base-200">
									{#each cliModels as model (model.id)}
										<div class="flex items-center justify-between px-4 py-2 bg-base-100">
											<div class="flex-1">
												<div class="flex items-center gap-2">
													<button
														type="button"
														class="btn btn-ghost btn-xs p-0 min-h-0 h-auto"
														onclick={() => togglePreferred('claude-cli', model)}
														title={model.preferred ? 'Remove preferred' : 'Set as preferred'}
													>
														<Star class="w-4 h-4 {model.preferred ? 'text-warning fill-warning' : 'text-base-content/30'}" />
													</button>
													<p class="font-medium text-sm text-base-content">{model.displayName}</p>
												</div>
												<div class="flex gap-2 flex-wrap mt-1">
													{#each kindOptions as kind}
														<label class="flex items-center gap-1 cursor-pointer">
															<input
																type="checkbox"
																class="checkbox checkbox-xs checkbox-primary"
																checked={(model.kind || []).includes(kind)}
																onchange={() => toggleKind('claude-cli', model, kind)}
															/>
															<span class="text-xs text-base-content/70">{kind}</span>
														</label>
													{/each}
												</div>
											</div>
											<Toggle
												checked={model.isActive}
												onchange={() => toggleModel('claude-cli', model)}
											/>
										</div>
									{/each}
								</div>
							{/if}
						</div>
					{/if}

					{#if availableCLIs.codex}
						{@const cliModels = getCLIModels('codex-cli')}
						<div class="border border-base-300 rounded-lg overflow-hidden">
							<button
								type="button"
								class="w-full flex items-center justify-between py-3 px-4 bg-base-200/30 hover:bg-base-200/50 transition-colors cursor-pointer"
								onclick={() => openCLIModal('codex')}
							>
								<div class="flex items-center gap-3">
									<div class="w-8 h-8 rounded-lg bg-success/10 flex items-center justify-center">
										<CheckCircle class="w-4 h-4 text-success" />
									</div>
									<div class="text-left">
										<p class="font-medium text-base-content">Codex CLI</p>
										<p class="text-xs text-base-content/60">OpenAI's coding assistant</p>
									</div>
								</div>
								<span class="badge badge-success badge-sm">Available</span>
							</button>
							{#if cliModels.length > 0}
								<div class="divide-y divide-base-200">
									{#each cliModels as model (model.id)}
										<div class="flex items-center justify-between px-4 py-2 bg-base-100">
											<div class="flex-1">
												<div class="flex items-center gap-2">
													<button
														type="button"
														class="btn btn-ghost btn-xs p-0 min-h-0 h-auto"
														onclick={() => togglePreferred('codex-cli', model)}
														title={model.preferred ? 'Remove preferred' : 'Set as preferred'}
													>
														<Star class="w-4 h-4 {model.preferred ? 'text-warning fill-warning' : 'text-base-content/30'}" />
													</button>
													<p class="font-medium text-sm text-base-content">{model.displayName}</p>
												</div>
												<div class="flex gap-2 flex-wrap mt-1">
													{#each kindOptions as kind}
														<label class="flex items-center gap-1 cursor-pointer">
															<input
																type="checkbox"
																class="checkbox checkbox-xs checkbox-primary"
																checked={(model.kind || []).includes(kind)}
																onchange={() => toggleKind('codex-cli', model, kind)}
															/>
															<span class="text-xs text-base-content/70">{kind}</span>
														</label>
													{/each}
												</div>
											</div>
											<Toggle
												checked={model.isActive}
												onchange={() => toggleModel('codex-cli', model)}
											/>
										</div>
									{/each}
								</div>
							{/if}
						</div>
					{/if}

					{#if availableCLIs.gemini}
						{@const cliModels = getCLIModels('gemini-cli')}
						<div class="border border-base-300 rounded-lg overflow-hidden">
							<button
								type="button"
								class="w-full flex items-center justify-between py-3 px-4 bg-base-200/30 hover:bg-base-200/50 transition-colors cursor-pointer"
								onclick={() => openCLIModal('gemini')}
							>
								<div class="flex items-center gap-3">
									<div class="w-8 h-8 rounded-lg bg-success/10 flex items-center justify-center">
										<CheckCircle class="w-4 h-4 text-success" />
									</div>
									<div class="text-left">
										<p class="font-medium text-base-content">Gemini CLI</p>
										<p class="text-xs text-base-content/60">Google's coding assistant</p>
									</div>
								</div>
								<span class="badge badge-success badge-sm">Available</span>
							</button>
							{#if cliModels.length > 0}
								<div class="divide-y divide-base-200">
									{#each cliModels as model (model.id)}
										<div class="flex items-center justify-between px-4 py-2 bg-base-100">
											<div class="flex-1">
												<div class="flex items-center gap-2">
													<button
														type="button"
														class="btn btn-ghost btn-xs p-0 min-h-0 h-auto"
														onclick={() => togglePreferred('gemini-cli', model)}
														title={model.preferred ? 'Remove preferred' : 'Set as preferred'}
													>
														<Star class="w-4 h-4 {model.preferred ? 'text-warning fill-warning' : 'text-base-content/30'}" />
													</button>
													<p class="font-medium text-sm text-base-content">{model.displayName}</p>
												</div>
												<div class="flex gap-2 flex-wrap mt-1">
													{#each kindOptions as kind}
														<label class="flex items-center gap-1 cursor-pointer">
															<input
																type="checkbox"
																class="checkbox checkbox-xs checkbox-primary"
																checked={(model.kind || []).includes(kind)}
																onchange={() => toggleKind('gemini-cli', model, kind)}
															/>
															<span class="text-xs text-base-content/70">{kind}</span>
														</label>
													{/each}
												</div>
											</div>
											<Toggle
												checked={model.isActive}
												onchange={() => toggleModel('gemini-cli', model)}
											/>
										</div>
									{/each}
								</div>
							{/if}
						</div>
					{/if}
				</div>

				<div class="mt-4 p-3 bg-base-200/50 rounded-lg">
					<p class="text-xs text-base-content/70">
						CLI providers are automatically detected. Click on a provider to configure it.
					</p>
				</div>
			</Card>
		{/if}

		<!-- Task Routing Configuration -->
		<Card>
			<div class="flex items-center justify-between mb-4">
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-xl bg-secondary/10 flex items-center justify-center">
						<Cpu class="w-5 h-5 text-secondary" />
					</div>
					<div>
						<h3 class="text-lg font-semibold text-base-content">Task-Based Model Routing</h3>
						<p class="text-sm text-base-content/60">Assign specific models to different task types</p>
					</div>
				</div>
				<Button type="ghost" size="sm" onclick={() => (showRoutingConfig = !showRoutingConfig)}>
					{showRoutingConfig ? 'Hide' : 'Configure'}
				</Button>
			</div>

			{#if !showRoutingConfig && taskRouting}
				<div class="grid grid-cols-2 md:grid-cols-5 gap-3">
					<div class="bg-base-200/50 rounded-lg p-3">
						<div class="flex items-center gap-2 mb-1">
							<Eye class="w-4 h-4 text-info" />
							<span class="text-xs font-medium text-base-content/70">Vision</span>
						</div>
						<p class="text-sm text-base-content truncate">{taskRouting.vision || 'Auto'}</p>
					</div>
					<div class="bg-base-200/50 rounded-lg p-3">
						<div class="flex items-center gap-2 mb-1">
							<Volume2 class="w-4 h-4 text-warning" />
							<span class="text-xs font-medium text-base-content/70">Audio</span>
						</div>
						<p class="text-sm text-base-content truncate">{taskRouting.audio || 'Auto'}</p>
					</div>
					<div class="bg-base-200/50 rounded-lg p-3">
						<div class="flex items-center gap-2 mb-1">
							<Brain class="w-4 h-4 text-secondary" />
							<span class="text-xs font-medium text-base-content/70">Reasoning</span>
						</div>
						<p class="text-sm text-base-content truncate">{taskRouting.reasoning || 'Auto'}</p>
					</div>
					<div class="bg-base-200/50 rounded-lg p-3">
						<div class="flex items-center gap-2 mb-1">
							<Code class="w-4 h-4 text-accent" />
							<span class="text-xs font-medium text-base-content/70">Code</span>
						</div>
						<p class="text-sm text-base-content truncate">{taskRouting.code || 'Auto'}</p>
					</div>
					<div class="bg-base-200/50 rounded-lg p-3">
						<div class="flex items-center gap-2 mb-1">
							<Sparkles class="w-4 h-4 text-primary" />
							<span class="text-xs font-medium text-base-content/70">General</span>
						</div>
						<p class="text-sm text-base-content truncate">{taskRouting.general || 'Auto'}</p>
					</div>
				</div>
			{/if}

			{#if showRoutingConfig}
				{@const modelOptions = getAllModelOptions()}
				<div class="space-y-4">
					<div class="grid grid-cols-1 md:grid-cols-2 gap-4">
						<div>
							<label for="routing-vision" class="flex items-center gap-2 text-sm font-medium text-base-content mb-1">
								<Eye class="w-4 h-4 text-info" />
								Vision Tasks
							</label>
							<select id="routing-vision" bind:value={routingForm.vision} class="select select-bordered select-sm w-full">
								<option value="">Auto (best available)</option>
								{#each modelOptions as opt}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
							<p class="text-xs text-base-content/50 mt-1">Images, screenshots, visual analysis</p>
						</div>

						<div>
							<label for="routing-audio" class="flex items-center gap-2 text-sm font-medium text-base-content mb-1">
								<Volume2 class="w-4 h-4 text-warning" />
								Audio Tasks
							</label>
							<select id="routing-audio" bind:value={routingForm.audio} class="select select-bordered select-sm w-full">
								<option value="">Auto (best available)</option>
								{#each modelOptions as opt}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
							<p class="text-xs text-base-content/50 mt-1">Voice, transcription, audio analysis</p>
						</div>

						<div>
							<label for="routing-reasoning" class="flex items-center gap-2 text-sm font-medium text-base-content mb-1">
								<Brain class="w-4 h-4 text-secondary" />
								Reasoning Tasks
							</label>
							<select id="routing-reasoning" bind:value={routingForm.reasoning} class="select select-bordered select-sm w-full">
								<option value="">Auto (best available)</option>
								{#each modelOptions as opt}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
							<p class="text-xs text-base-content/50 mt-1">Complex analysis, problem solving</p>
						</div>

						<div>
							<label for="routing-code" class="flex items-center gap-2 text-sm font-medium text-base-content mb-1">
								<Code class="w-4 h-4 text-accent" />
								Code Tasks
							</label>
							<select id="routing-code" bind:value={routingForm.code} class="select select-bordered select-sm w-full">
								<option value="">Auto (best available)</option>
								{#each modelOptions as opt}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
							<p class="text-xs text-base-content/50 mt-1">Writing, debugging, refactoring code</p>
						</div>

						<div>
							<label for="routing-general" class="flex items-center gap-2 text-sm font-medium text-base-content mb-1">
								<Sparkles class="w-4 h-4 text-primary" />
								General Tasks
							</label>
							<select id="routing-general" bind:value={routingForm.general} class="select select-bordered select-sm w-full">
								<option value="">Auto (best available)</option>
								{#each modelOptions as opt}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
							<p class="text-xs text-base-content/50 mt-1">Chat, Q&A, general conversation</p>
						</div>
					</div>

					<div class="flex gap-2 justify-end pt-2">
						<Button type="ghost" size="sm" onclick={() => (showRoutingConfig = false)}>Cancel</Button>
						<Button type="primary" size="sm" onclick={saveTaskRouting} disabled={isSavingRouting}>
							{#if isSavingRouting}
								<Spinner size={16} />
								Saving...
							{:else}
								Save Routing
							{/if}
						</Button>
					</div>
				</div>
			{/if}
		</Card>

		<!-- Model Aliases Configuration -->
		<Card>
			<div class="flex items-center justify-between mb-4">
				<div class="flex items-center gap-3">
					<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center">
						<Tag class="w-5 h-5 text-accent" />
					</div>
					<div>
						<h3 class="text-lg font-semibold text-base-content">Model Aliases</h3>
						<p class="text-sm text-base-content/60">Create shortcuts like "fast" → haiku, "smart" → opus</p>
					</div>
				</div>
				<Button type="ghost" size="sm" onclick={() => (showAliasesConfig = !showAliasesConfig)}>
					{showAliasesConfig ? 'Hide' : 'Configure'}
				</Button>
			</div>

			{#if !showAliasesConfig && aliases.length > 0}
				<div class="flex flex-wrap gap-2">
					{#each aliases as alias}
						<div class="badge badge-outline gap-1">
							<span class="font-medium">{alias.alias}</span>
							<span class="text-base-content/50">→</span>
							<span class="text-xs">{alias.modelId.split('/').pop()}</span>
						</div>
					{/each}
				</div>
			{:else if !showAliasesConfig}
				<p class="text-sm text-base-content/50">No custom aliases configured. Using defaults.</p>
			{/if}

			{#if showAliasesConfig}
				{@const modelOptions = getAllModelOptions()}
				<div class="space-y-4">
					<p class="text-sm text-base-content/70">
						Define shortcuts you can use when switching models. Say "use fast" to switch to your fast model.
					</p>

					<div class="space-y-2">
						{#each aliasesForm as aliasEntry, index}
							<div class="flex items-center gap-2">
								<input
									type="text"
									placeholder="Alias (e.g., fast)"
									bind:value={aliasEntry.alias}
									class="input input-bordered input-sm w-32"
								/>
								<span class="text-base-content/50">→</span>
								<select bind:value={aliasEntry.modelId} class="select select-bordered select-sm flex-1">
									<option value="">Select model...</option>
									{#each modelOptions as opt}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
								<button
									type="button"
									class="btn btn-ghost btn-sm btn-square"
									onclick={() => removeAlias(index)}
								>
									<Trash2 class="w-4 h-4" />
								</button>
							</div>
						{/each}
					</div>

					<Button type="ghost" size="sm" onclick={addAlias}>
						<Plus class="w-4 h-4" />
						Add Alias
					</Button>

					<div class="flex gap-2 justify-end pt-2">
						<Button type="ghost" size="sm" onclick={() => (showAliasesConfig = false)}>Cancel</Button>
						<Button type="primary" size="sm" onclick={saveAliases} disabled={isSavingAliases}>
							{#if isSavingAliases}
								<Spinner size={16} />
								Saving...
							{:else}
								Save Aliases
							{/if}
						</Button>
					</div>
				</div>
			{/if}
		</Card>

		<!-- Info Card -->
		<Card>
			<div class="bg-base-200 rounded-lg p-4">
				<h4 class="font-medium text-base-content mb-2">How it works</h4>
				<ul class="text-sm text-base-content/70 space-y-1 list-disc list-inside">
					<li>Toggle models on/off to control what GoBot can use</li>
					<li>Use checkboxes to set <span class="badge badge-xs badge-primary badge-outline">kind</span> tags like fast, smart, code, cheap</li>
					<li>Click <Star class="w-3 h-3 text-warning fill-warning inline" /> to mark your preferred model for each kind</li>
					<li>Say "use fast" to switch to your preferred model with that kind</li>
					<li>If one provider fails, GoBot falls back to other models with the same kind</li>
				</ul>
			</div>
		</Card>
	{/if}
</div>

<!-- CLI Provider Configuration Modal -->
{#if showCLIModal && selectedCLI}
	{@const cli = selectedCLI}
	{@const cliModels = getCLIModels(cli.id)}
	<div class="modal modal-open">
		<div class="modal-box">
			<h3 class="font-bold text-lg mb-4">{cli.name} Configuration</h3>

			<div class="space-y-4">
				<div class="flex items-center gap-3 p-3 bg-base-200 rounded-lg">
					<Terminal class="w-5 h-5 text-accent" />
					<div>
						<p class="font-medium text-sm">Command: <code class="text-accent">{cli.command}</code></p>
						<p class="text-xs text-base-content/60">Install: {cli.installHint}</p>
					</div>
				</div>

				{#if cliModels.length > 0}
					<div>
						<h4 class="font-medium text-sm mb-2">Available Models</h4>
						<div class="space-y-2">
							{#each cliModels as model (model.id)}
								<div class="flex items-center justify-between p-3 bg-base-200/50 rounded-lg">
									<div class="flex-1">
										<div class="flex items-center gap-2">
											<button
												type="button"
												class="btn btn-ghost btn-xs p-0 min-h-0 h-auto"
												onclick={() => togglePreferred(cli.id, model)}
												title={model.preferred ? 'Remove preferred' : 'Set as preferred'}
											>
												<Star class="w-4 h-4 {model.preferred ? 'text-warning fill-warning' : 'text-base-content/30'}" />
											</button>
											<span class="font-medium text-sm">{model.displayName}</span>
										</div>
										<div class="flex gap-2 flex-wrap mt-1">
											{#each kindOptions as kind}
												<label class="flex items-center gap-1 cursor-pointer">
													<input
														type="checkbox"
														class="checkbox checkbox-xs checkbox-primary"
														checked={(model.kind || []).includes(kind)}
														onchange={() => toggleKind(cli.id, model, kind)}
													/>
													<span class="text-xs text-base-content/70">{kind}</span>
												</label>
											{/each}
										</div>
									</div>
									<Toggle
										checked={model.isActive}
										onchange={() => toggleModel(cli.id, model)}
									/>
								</div>
							{/each}
						</div>
					</div>
				{:else}
					<div class="text-center py-4">
						<p class="text-sm text-base-content/60">No models configured for this CLI provider.</p>
						<p class="text-xs text-base-content/50 mt-1">Models are defined in models.yaml</p>
					</div>
				{/if}
			</div>

			<div class="modal-action">
				<Button type="ghost" onclick={closeCLIModal}>Close</Button>
			</div>
		</div>
		<div class="modal-backdrop" onclick={closeCLIModal}></div>
	</div>
{/if}
