<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Modal from '$lib/components/ui/Modal.svelte';
	import AlertDialog from '$lib/components/ui/AlertDialog.svelte';
	import Badge from '$lib/components/ui/Badge.svelte';
	import Radio from '$lib/components/ui/Radio.svelte';
	import {
		Plus,
		CheckCircle,
		XCircle,
		RefreshCw,
		Play,
		MoreVertical,
		Server
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { MCPIntegration } from '$lib/api/nebo';

	let integrations = $state<MCPIntegration[]>([]);
	let isLoading = $state(true);
	let showAddModal = $state(false);
	let addStep = $state<'url' | 'auth' | 'name'>('url');

	// Add form state
	let newServerUrl = $state('');
	let newAuthType = $state<'oauth' | 'api_key' | 'none'>('oauth');
	let newApiKey = $state('');
	let newName = $state('');
	let isAdding = $state(false);

	// Dropdown state
	let openDropdown = $state<string | null>(null);

	// Dialog state
	let showDeleteDialog = $state(false);
	let deleteTarget = $state<MCPIntegration | null>(null);
	let showAlertDialog = $state(false);
	let alertMessage = $state('');

	onMount(async () => {
		await loadIntegrations();

		// Check for OAuth callback success/error
		const connected = $page.url.searchParams.get('connected');
		const error = $page.url.searchParams.get('error');
		if (connected) {
			await loadIntegrations();
		}
		if (error) {
			const desc = $page.url.searchParams.get('error_description') || error;
			alertMessage = `OAuth error: ${desc}`;
			showAlertDialog = true;
		}
	});

	async function loadIntegrations() {
		isLoading = true;
		try {
			const data = await api.listMCPIntegrations();
			integrations = data.integrations || [];
		} catch (error) {
			console.error('Failed to load integrations:', error);
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
		showAddModal = true;
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
						window.location.href = oauthResult.authUrl;
						return;
					}
				} catch (e) {
					console.error('OAuth flow failed:', e);
				}
			}

			await loadIntegrations();
			showAddModal = false;
		} catch (error) {
			console.error('Failed to add server:', error);
		} finally {
			isAdding = false;
		}
	}

	function confirmDelete(integration: MCPIntegration) {
		deleteTarget = integration;
		showDeleteDialog = true;
		openDropdown = null;
	}

	async function executeDelete() {
		if (!deleteTarget) return;
		try {
			await api.deleteMCPIntegration(deleteTarget.id);
			integrations = integrations.filter((i) => i.id !== deleteTarget!.id);
		} catch (error) {
			console.error('Failed to delete integration:', error);
		}
		deleteTarget = null;
	}

	async function testIntegration(integration: MCPIntegration) {
		try {
			const result = await api.testMCPIntegration(integration.id);
			if (result.success) {
				await loadIntegrations();
			} else {
				alertMessage = result.message || 'Connection test failed';
				showAlertDialog = true;
			}
		} catch (error) {
			console.error('Failed to test integration:', error);
		}
	}

	async function toggleIntegration(integration: MCPIntegration) {
		try {
			await api.updateMCPIntegration({ isEnabled: !integration.isEnabled }, integration.id);
			await loadIntegrations();
		} catch (error) {
			console.error('Failed to toggle integration:', error);
		}
		openDropdown = null;
	}

	function toggleDropdown(id: string) {
		openDropdown = openDropdown === id ? null : id;
	}

	function authBadgeVariant(authType: string): 'info' | 'warning' | 'ghost' {
		if (authType === 'oauth') return 'info';
		if (authType === 'api_key') return 'warning';
		return 'ghost';
	}

	function authBadgeLabel(authType: string): string {
		if (authType === 'oauth') return 'OAUTH';
		if (authType === 'api_key') return 'API KEY';
		return 'NONE';
	}

	let isUrlValid = $derived(() => {
		try {
			const u = new URL(newServerUrl);
			return u.protocol === 'http:' || u.protocol === 'https:';
		} catch {
			return false;
		}
	});
</script>

<!-- Header -->
<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">Integrations</h2>
		<p class="text-sm text-base-content/60">Connect to external services and MCP servers</p>
	</div>
	<div class="flex gap-2">
		<Button type="ghost" onclick={loadIntegrations}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
		<Button type="primary" onclick={openAddModal}>
			<Plus class="w-4 h-4 mr-2" />
			Add Server
		</Button>
	</div>
</div>

<!-- Server List -->
<Card>
	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading servers...</div>
	{:else if integrations.length === 0}
		<div class="py-12 text-center">
			<Server class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
			<h3 class="font-display font-bold text-base-content mb-2">No integrations connected</h3>
			<p class="text-base-content/60 mb-4">
				Connect to external MCP servers to extend your agent's capabilities
			</p>
			<Button type="primary" onclick={openAddModal}>
				<Plus class="w-4 h-4 mr-2" />
				Add Server
			</Button>
		</div>
	{:else}
		<div class="space-y-2">
			{#each integrations as integration}
				<div class="flex items-center justify-between p-4 rounded-lg bg-base-200">
					<div class="flex items-center gap-3">
						<div
							class="w-10 h-10 rounded-lg bg-primary/10 flex items-center justify-center text-primary font-bold text-lg"
						>
							{integration.name.charAt(0).toUpperCase()}
						</div>
						<div>
							<div class="flex items-center gap-2">
								<span class="font-medium">{integration.name}</span>
								<Badge variant={authBadgeVariant(integration.authType)} size="xs">
									{authBadgeLabel(integration.authType)}
								</Badge>
							</div>
							<div class="text-xs text-base-content/50">
								{integration.serverUrl || integration.serverType}
							</div>
							<div class="flex items-center gap-1 text-xs mt-0.5">
								{#if integration.connectionStatus === 'connected'}
									<CheckCircle class="w-3 h-3 text-success" />
									<span class="text-success">Connected</span>
									{#if integration.toolCount > 0}
										<span class="text-base-content/50 ml-1"
											>&middot; {integration.toolCount} tools</span
										>
									{/if}
								{:else if integration.connectionStatus === 'error'}
									<XCircle class="w-3 h-3 text-error" />
									<span class="text-error">Error</span>
								{:else}
									<XCircle class="w-3 h-3 text-base-content/40" />
									<span class="text-base-content/40">Disconnected</span>
								{/if}
								{#if integration.lastError}
									<span class="text-error/60 ml-2">&middot; {integration.lastError}</span>
								{/if}
							</div>
						</div>
					</div>
					<div class="flex items-center gap-1">
						<Button type="ghost" size="sm" onclick={() => testIntegration(integration)}>
							<Play class="w-3 h-3 mr-1" />
							Test
						</Button>
						<div class="relative">
							<button
								onclick={() => toggleDropdown(integration.id)}
								class="btn btn-ghost btn-sm btn-square"
							>
								<MoreVertical class="w-4 h-4" />
							</button>
							{#if openDropdown === integration.id}
								<button
									class="fixed inset-0 z-40 cursor-default"
									onclick={() => (openDropdown = null)}
									aria-label="Close menu"
								></button>
								<div
									class="absolute right-0 top-full mt-1 z-50 bg-base-100 rounded-lg shadow-lg border border-base-300 py-1 min-w-[140px]"
								>
									<button
										onclick={() => toggleIntegration(integration)}
										class="w-full text-left px-4 py-2 text-sm hover:bg-base-200"
									>
										{integration.isEnabled ? 'Disable' : 'Enable'}
									</button>
									<button
										onclick={() => confirmDelete(integration)}
										class="w-full text-left px-4 py-2 text-sm text-error hover:bg-error/10"
									>
										Delete
									</button>
								</div>
							{/if}
						</div>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</Card>

<!-- Add Server Modal -->
<Modal bind:show={showAddModal} title="Add Integration" size="md" closeOnBackdrop>
	<div class="space-y-4">
		{#if addStep === 'url'}
			<div>
				<label for="server-url" class="block text-sm font-medium mb-1">Server URL</label>
				<input
					id="server-url"
					type="url"
					bind:value={newServerUrl}
					placeholder="https://example.com/mcp"
					class="input input-bordered w-full"
					onkeydown={(e) => {
						if (e.key === 'Enter' && isUrlValid()) nextStep();
					}}
				/>
				<p class="text-xs text-base-content/50 mt-1">
					The MCP server's endpoint URL (Streamable HTTP)
				</p>
			</div>
		{:else if addStep === 'auth'}
			<div>
				<p class="text-sm font-medium mb-3">Authentication</p>
				<div class="space-y-1">
					<Radio
						name="auth-type"
						value="oauth"
						checked={newAuthType === 'oauth'}
						label="OAuth"
						description="Authenticate via OAuth 2.1 (recommended)"
						onchange={() => (newAuthType = 'oauth')}
					/>
					<Radio
						name="auth-type"
						value="api_key"
						checked={newAuthType === 'api_key'}
						label="API Key / Bearer Token"
						description="Authenticate with a static token"
						onchange={() => (newAuthType = 'api_key')}
					/>
					<Radio
						name="auth-type"
						value="none"
						checked={newAuthType === 'none'}
						label="None"
						description="No authentication required"
						onchange={() => (newAuthType = 'none')}
					/>
				</div>
				{#if newAuthType === 'api_key'}
					<div class="mt-4">
						<label for="api-key" class="block text-sm font-medium mb-1">API Key</label>
						<input
							id="api-key"
							type="password"
							bind:value={newApiKey}
							placeholder="Enter API key or bearer token"
							class="input input-bordered w-full"
						/>
					</div>
				{/if}
			</div>
		{:else if addStep === 'name'}
			<div>
				<label for="server-name" class="block text-sm font-medium mb-1">Name (optional)</label>
				<input
					id="server-name"
					type="text"
					bind:value={newName}
					placeholder={hostnameFromUrl(newServerUrl) || 'MCP Server'}
					class="input input-bordered w-full"
				/>
				<p class="text-xs text-base-content/50 mt-1">
					A friendly name for this server. Defaults to the hostname.
				</p>
			</div>

			<div class="bg-base-200 rounded-lg p-3 text-sm space-y-1">
				<div class="flex justify-between">
					<span class="text-base-content/60">URL</span>
					<span class="font-mono text-xs">{newServerUrl}</span>
				</div>
				<div class="flex justify-between">
					<span class="text-base-content/60">Auth</span>
					<span>{authBadgeLabel(newAuthType)}</span>
				</div>
			</div>
		{/if}
	</div>

	{#snippet footer()}
		<div class="flex gap-2 w-full">
			{#if addStep === 'url'}
				<Button type="ghost" class="flex-1" onclick={() => (showAddModal = false)}>Cancel</Button>
				<Button type="primary" class="flex-1" onclick={nextStep} disabled={!isUrlValid()}>
					Next
				</Button>
			{:else if addStep === 'auth'}
				<Button type="ghost" class="flex-1" onclick={prevStep}>Back</Button>
				<Button
					type="primary"
					class="flex-1"
					onclick={nextStep}
					disabled={newAuthType === 'api_key' && !newApiKey}
				>
					Next
				</Button>
			{:else}
				<Button type="ghost" class="flex-1" onclick={prevStep}>Back</Button>
				<Button type="primary" class="flex-1" onclick={addServer} disabled={isAdding}>
					{#if newAuthType === 'oauth'}
						Connect with OAuth
					{:else}
						Add Server
					{/if}
				</Button>
			{/if}
		</div>
	{/snippet}
</Modal>

<!-- Delete Confirmation -->
<AlertDialog
	bind:open={showDeleteDialog}
	title="Remove Server"
	description="Are you sure you want to remove {deleteTarget?.name}? This will disconnect the server and unregister its tools."
	actionLabel="Remove"
	actionType="danger"
	onAction={executeDelete}
/>

<!-- Alert Dialog -->
<AlertDialog
	bind:open={showAlertDialog}
	title="Error"
	description={alertMessage}
	actionLabel="OK"
	cancelLabel="Dismiss"
/>
