<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import {
		Plus,
		CheckCircle,
		XCircle,
		RefreshCw,
		Trash2,
		Server,
		X
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { MCPIntegration } from '$lib/api/nebo';

	let integrations = $state<MCPIntegration[]>([]);
	let isLoading = $state(true);
	let error = $state('');
	let testingId = $state<string | null>(null);
	let testResult = $state<{ id: string; success: boolean; message: string } | null>(null);
	let connectingId = $state<string | null>(null);

	// Add form state
	let showAddForm = $state(false);
	let addStep = $state<'url' | 'auth' | 'name'>('url');
	let newServerUrl = $state('');
	let newAuthType = $state<'oauth' | 'api_key' | 'none'>('oauth');
	let newApiKey = $state('');
	let newName = $state('');
	let isAdding = $state(false);
	let addError = $state('');

	onMount(async () => {
		await loadIntegrations();

		// Check for OAuth callback success/error
		const connected = $page.url.searchParams.get('connected');
		const oauthError = $page.url.searchParams.get('error');
		if (connected) {
			await loadIntegrations();
		}
		if (oauthError) {
			const desc = $page.url.searchParams.get('error_description') || oauthError;
			error = `OAuth error: ${desc}`;
		}
	});

	async function loadIntegrations() {
		isLoading = true;
		error = '';
		try {
			const data = await api.listMCPIntegrations();
			integrations = data.integrations || [];
		} catch (err: any) {
			error = err?.message || 'Failed to load integrations';
		} finally {
			isLoading = false;
		}
	}

	function hostnameFromUrl(url: string): string {
		try {
			return new URL(url).hostname;
		} catch {
			return '';
		}
	}

	function openAddModal() {
		newServerUrl = '';
		newAuthType = 'oauth';
		newApiKey = '';
		newName = '';
		addStep = 'url';
		isAdding = false;
		addError = '';
		showAddForm = true;
	}

	function closeAddModal() {
		showAddForm = false;
		newServerUrl = '';
		newAuthType = 'oauth';
		newApiKey = '';
		newName = '';
		addError = '';
	}

	function nextStep() {
		if (addStep === 'url') {
			addStep = 'auth';
		} else if (addStep === 'auth') {
			if (!newName) {
				newName = hostnameFromUrl(newServerUrl);
			}
			addStep = 'name';
		}
	}

	function prevStep() {
		if (addStep === 'name') {
			addStep = 'auth';
		} else if (addStep === 'auth') {
			addStep = 'url';
		}
	}

	async function addServer() {
		isAdding = true;
		addError = '';
		try {
			const result = await api.createMCPIntegration({
				name: newName || hostnameFromUrl(newServerUrl) || 'MCP Server',
				serverUrl: newServerUrl,
				authType: newAuthType,
				apiKey: newAuthType === 'api_key' ? newApiKey : undefined
			});

			if (newAuthType === 'oauth' && result.integration?.id) {
				try {
					const oauthResult = await api.getMCPOAuthURL(result.integration.id);
					if (oauthResult.authUrl) {
						// Open OAuth in system browser — don't navigate the Tauri webview away
						window.open(oauthResult.authUrl, '_blank');
						closeAddModal();
						// Poll for completion (callback updates DB status)
						const integrationId = result.integration.id;
						const pollInterval = setInterval(async () => {
							try {
								const data = await api.listMCPIntegrations();
								const updated = (data.integrations || []).find((i: any) => i.id === integrationId);
								if (updated && (updated.connectionStatus === 'connected' || updated.connectionStatus === 'disconnected' && updated.lastConnectedAt)) {
									clearInterval(pollInterval);
									await loadIntegrations();
								}
							} catch { /* ignore */ }
						}, 2000);
						// Stop polling after 3 minutes
						setTimeout(() => {
							clearInterval(pollInterval);
							loadIntegrations();
						}, 180000);
						return;
					}
				} catch (e: any) {
					addError = e?.message || 'OAuth flow failed';
					return;
				}
			}

			await loadIntegrations();
			closeAddModal();
		} catch (err: any) {
			addError = err?.message || 'Failed to add server';
		} finally {
			isAdding = false;
		}
	}

	async function deleteIntegration(integration: MCPIntegration) {
		try {
			await api.deleteMCPIntegration(integration.id);
			integrations = integrations.filter((i) => i.id !== integration.id);
		} catch (err: any) {
			error = err?.message || 'Failed to delete integration';
		}
	}

	async function testIntegration(id: string) {
		testingId = id;
		testResult = null;
		try {
			const result = await api.testMCPIntegration(id);
			testResult = { id, success: result.success, message: result.message };
			if (result.success) {
				await loadIntegrations();
			}
		} catch (err: any) {
			testResult = { id, success: false, message: err?.message || 'Test failed' };
		} finally {
			testingId = null;
		}
	}

	async function connectIntegration(id: string) {
		connectingId = id;
		testResult = null;
		try {
			const result = await api.connectMCPIntegration(id);
			testResult = { id, success: result.success, message: result.message };
			await loadIntegrations();
		} catch (err: any) {
			testResult = { id, success: false, message: err?.message || 'Connection failed' };
		} finally {
			connectingId = null;
		}
	}

	async function toggleIntegration(integration: MCPIntegration) {
		const newEnabled = !integration.isEnabled;
		integration.isEnabled = newEnabled;
		try {
			await api.updateMCPIntegration({ isEnabled: newEnabled }, integration.id);
			await loadIntegrations();
		} catch (err: any) {
			integration.isEnabled = !newEnabled;
			error = err?.message || 'Failed to update integration';
		}
	}

	let isUrlValid = $derived(() => {
		try {
			const u = new URL(newServerUrl);
			return u.protocol === 'http:' || u.protocol === 'https:';
		} catch {
			return false;
		}
	});

	function authLabel(authType: string): string {
		if (authType === 'oauth') return 'OAuth';
		if (authType === 'api_key') return 'API Key';
		return 'None';
	}
</script>

<div class="max-w-3xl">
	<div class="mb-6">
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Integrations</h2>
		<p class="text-base text-base-content/80">Connect to external MCP servers to extend capabilities</p>
	</div>

	{#if isLoading}
		<div class="flex items-center justify-center gap-3 py-16">
			<Spinner size={20} />
			<span class="text-base text-base-content/80">Loading integrations...</span>
		</div>
	{:else}
		<div class="space-y-6">
			{#if error}
				<Alert type="error" title="Error">{error}</Alert>
			{/if}

			<!-- MCP Servers -->
			<section>
				<div class="flex items-center justify-between mb-3">
					<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider">MCP Servers</h3>
					<div class="flex items-center gap-2">
						<button
							type="button"
							class="text-base text-base-content/80 hover:text-primary transition-colors"
							onclick={loadIntegrations}
							aria-label="Refresh"
						>
							<RefreshCw class="w-4 h-4" />
						</button>
						<button
							type="button"
							class="flex items-center gap-1.5 text-base font-medium text-base-content/80 hover:text-primary transition-colors"
							onclick={openAddModal}
						>
							<Plus class="w-4 h-4" /> Add server
						</button>
					</div>
				</div>

				<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
					{#if integrations.length === 0}
						<div class="py-8 text-center">
							<Server class="w-10 h-10 mx-auto mb-3 text-base-content/40" />
							<p class="text-base text-base-content/80 mb-4">No MCP servers connected</p>
							<button
								type="button"
								class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all"
								onclick={openAddModal}
							>
								Add Server
							</button>
						</div>
					{:else}
						<div class="space-y-2">
							{#each integrations as integration (integration.id)}
								<div class="rounded-xl bg-base-content/5 border border-base-content/10 py-2.5 px-4">
									<div class="flex items-center justify-between">
										<div class="flex items-center gap-3 min-w-0">
											<div class="w-9 h-9 rounded-lg bg-primary/10 flex items-center justify-center text-primary font-bold text-base shrink-0">
												{integration.name.charAt(0).toUpperCase()}
											</div>
											<div class="min-w-0">
												<div class="flex items-center gap-2">
													<p class="text-base font-medium text-base-content truncate">{integration.name}</p>
													<span class="text-xs font-medium px-2 py-0.5 rounded-full bg-base-content/10 text-base-content/60 shrink-0">
														{authLabel(integration.authType)}
													</span>
												</div>
												<div class="flex items-center gap-2 mt-0.5">
													<div class="flex items-center gap-1">
														{#if integration.connectionStatus === 'connected'}
															<CheckCircle class="w-3 h-3 text-success" />
															<span class="text-sm text-success">Connected</span>
														{:else if integration.connectionStatus === 'error'}
															<XCircle class="w-3 h-3 text-error" />
															<span class="text-sm text-error">Error</span>
														{:else}
															<XCircle class="w-3 h-3 text-base-content/40" />
															<span class="text-sm text-base-content/60">Disconnected</span>
														{/if}
													</div>
													{#if integration.connectionStatus === 'connected' && integration.toolCount > 0}
														<span class="text-sm text-base-content/60">&middot; {integration.toolCount} tools</span>
													{/if}
												</div>
											</div>
										</div>
										<div class="flex items-center gap-3 shrink-0">
											{#if integration.connectionStatus === 'connected'}
												<button
													type="button"
													class="text-base text-base-content/80 hover:text-primary transition-colors"
													onclick={() => testIntegration(integration.id)}
													disabled={testingId === integration.id}
												>
													{#if testingId === integration.id}<Spinner size={14} />{:else}Test{/if}
												</button>
											{:else}
												<button
													type="button"
													class="h-7 px-4 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all disabled:opacity-50"
													onclick={() => connectIntegration(integration.id)}
													disabled={connectingId === integration.id}
												>
													{#if connectingId === integration.id}<Spinner size={12} />{:else}Connect{/if}
												</button>
											{/if}
											<Toggle checked={integration.isEnabled} onchange={() => toggleIntegration(integration)} />
											<button
												type="button"
												class="text-base text-base-content/80 hover:text-error transition-colors"
												onclick={() => deleteIntegration(integration)}
											>
												<Trash2 class="w-4 h-4" />
											</button>
										</div>
									</div>
									{#if testResult?.id === integration.id}
										<p class="text-sm mt-1.5 {testResult.success ? 'text-success' : 'text-error'}">{testResult.message}</p>
									{/if}
									{#if integration.lastError && testResult?.id !== integration.id}
										<p class="text-sm text-error/70 mt-1.5">{integration.lastError}</p>
									{/if}
								</div>
							{/each}
						</div>
					{/if}
				</div>
			</section>
		</div>
	{/if}
</div>

<!-- Add Server Modal -->
{#if showAddForm}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="nebo-modal-backdrop" role="dialog" aria-modal="true" tabindex="-1" onkeydown={(e) => e.key === 'Escape' && closeAddModal()}>
		<button type="button" class="nebo-modal-overlay" onclick={closeAddModal}></button>
		<div class="nebo-modal-card max-w-lg">
			<!-- Header -->
			<div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
				<div>
					<h3 class="font-display text-lg font-bold">Add MCP Server</h3>
					<p class="text-base text-base-content/60 mt-0.5">
						{#if addStep === 'url'}Step 1 of 3 — Server URL{:else if addStep === 'auth'}Step 2 of 3 — Authentication{:else}Step 3 of 3 — Confirm{/if}
					</p>
				</div>
				<button type="button" onclick={closeAddModal} class="nebo-modal-close" aria-label="Close">
					<X class="w-5 h-5 text-base-content/90" />
				</button>
			</div>

			<!-- Body -->
			<div class="px-5 py-5 space-y-4">
				{#if addStep === 'url'}
					<div>
						<label class="text-base font-medium text-base-content/80" for="server-url">Server URL</label>
						<input
							id="server-url"
							type="url"
							bind:value={newServerUrl}
							placeholder="https://example.com/mcp"
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
							onkeydown={(e) => { if (e.key === 'Enter' && isUrlValid()) nextStep(); }}
						/>
						<p class="text-sm text-base-content/60 mt-1.5">The MCP server's endpoint URL (Streamable HTTP)</p>
					</div>
				{:else if addStep === 'auth'}
					<div>
						<p class="text-base font-medium text-base-content/80 mb-3">Authentication method</p>
						<div class="space-y-2">
							{#each [
								{ value: 'oauth', label: 'OAuth 2.1', desc: 'Recommended — secure token-based auth' },
								{ value: 'api_key', label: 'API Key / Bearer Token', desc: 'Authenticate with a static token' },
								{ value: 'none', label: 'None', desc: 'No authentication required' }
							] as opt}
								<label
									class="flex items-start gap-3 p-3 rounded-xl cursor-pointer transition-colors
										{newAuthType === opt.value ? 'bg-primary/10 ring-1 ring-primary/20' : 'bg-base-content/5 hover:bg-base-content/10'}"
								>
									<input
										type="radio"
										name="auth-type"
										value={opt.value}
										checked={newAuthType === opt.value}
										onchange={() => (newAuthType = opt.value as any)}
										class="radio radio-primary radio-sm mt-0.5"
									/>
									<div>
										<p class="text-base font-medium text-base-content">{opt.label}</p>
										<p class="text-sm text-base-content/60">{opt.desc}</p>
									</div>
								</label>
							{/each}
						</div>
						{#if newAuthType === 'api_key'}
							<div class="mt-4">
								<label class="text-base font-medium text-base-content/80" for="api-key">API Key</label>
								<input
									id="api-key"
									type="password"
									bind:value={newApiKey}
									placeholder="Enter API key or bearer token"
									class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
								/>
							</div>
						{/if}
					</div>
				{:else if addStep === 'name'}
					<div>
						<label class="text-base font-medium text-base-content/80" for="server-name">Name (optional)</label>
						<input
							id="server-name"
							type="text"
							bind:value={newName}
							placeholder={hostnameFromUrl(newServerUrl) || 'MCP Server'}
							class="w-full h-11 mt-2 rounded-xl bg-base-content/5 border border-base-content/10 px-4 text-base focus:outline-none focus:border-primary/50 transition-colors"
						/>
						<p class="text-sm text-base-content/60 mt-1.5">A friendly name for this server. Defaults to the hostname.</p>
					</div>

					<div class="rounded-xl bg-base-content/5 border border-base-content/10 p-4 space-y-2">
						<div class="flex justify-between text-base">
							<span class="text-base-content/60">URL</span>
							<span class="text-base-content font-mono text-sm truncate ml-4">{newServerUrl}</span>
						</div>
						<div class="flex justify-between text-base">
							<span class="text-base-content/60">Auth</span>
							<span class="text-base-content">{authLabel(newAuthType)}</span>
						</div>
					</div>

					{#if addError}
						<Alert type="error" title="Error">{addError}</Alert>
					{/if}
				{/if}
			</div>

			<!-- Footer -->
			<div class="flex items-center justify-end gap-3 px-5 py-4 border-t border-base-content/10">
				{#if addStep === 'url'}
					<button
						type="button"
						class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={closeAddModal}
					>
						Cancel
					</button>
					<button
						type="button"
						class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
						onclick={nextStep}
						disabled={!isUrlValid()}
					>
						Next
					</button>
				{:else if addStep === 'auth'}
					<button
						type="button"
						class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={prevStep}
					>
						Back
					</button>
					<button
						type="button"
						class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
						onclick={nextStep}
						disabled={newAuthType === 'api_key' && !newApiKey}
					>
						Next
					</button>
				{:else}
					<button
						type="button"
						class="h-10 px-5 rounded-full border border-base-content/10 text-base font-medium hover:bg-base-content/5 transition-colors"
						onclick={prevStep}
					>
						Back
					</button>
					<button
						type="button"
						class="h-10 px-6 rounded-full bg-primary text-primary-content text-base font-bold hover:brightness-110 transition-all disabled:opacity-30"
						onclick={addServer}
						disabled={isAdding}
					>
						{#if isAdding}<Spinner size={16} /> Connecting...{:else if newAuthType === 'oauth'}Connect with OAuth{:else}Add Server{/if}
					</button>
				{/if}
			</div>
		</div>
	</div>
{/if}
