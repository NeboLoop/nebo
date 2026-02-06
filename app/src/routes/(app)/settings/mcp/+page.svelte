<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import {
		Server,
		Plus,
		Trash2,
		CheckCircle,
		XCircle,
		RefreshCw,
		Link,
		ExternalLink,
		Play
	} from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { MCPIntegration, MCPServerInfo } from '$lib/api/nebo';

	let integrations = $state<MCPIntegration[]>([]);
	let registry = $state<MCPServerInfo[]>([]);
	let isLoading = $state(true);
	let showAddModal = $state(false);
	let selectedType = $state('notion');
	let newIntegration = $state({ name: '', serverUrl: '', apiKey: '' });

	onMount(async () => {
		await Promise.all([loadIntegrations(), loadRegistry()]);
	});

	async function loadRegistry() {
		try {
			const data = await api.listMCPServerRegistry();
			registry = data.servers || [];
			if (registry.length > 0) {
				selectedType = registry[0].id;
			}
		} catch (error) {
			console.error('Failed to load MCP registry:', error);
		}
	}

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

	async function addIntegration() {
		const serverInfo = registry.find((r) => r.id === selectedType);
		try {
			await api.createMCPIntegration({
				name: newIntegration.name || serverInfo?.name || selectedType,
				serverType: selectedType,
				serverUrl: newIntegration.serverUrl || undefined,
				authType: serverInfo?.authType || 'api_key',
				apiKey: newIntegration.apiKey || undefined
			});
			await loadIntegrations();
			showAddModal = false;
			newIntegration = { name: '', serverUrl: '', apiKey: '' };
		} catch (error) {
			console.error('Failed to add integration:', error);
		}
	}

	async function deleteIntegration(integration: MCPIntegration) {
		if (!confirm(`Remove ${integration.name}?`)) return;
		try {
			await api.deleteMCPIntegration(integration.id);
			integrations = integrations.filter((i) => i.id !== integration.id);
		} catch (error) {
			console.error('Failed to delete integration:', error);
		}
	}

	async function testIntegration(integration: MCPIntegration) {
		try {
			const result = await api.testMCPIntegration(integration.id);
			if (result.success) {
				await loadIntegrations();
			} else {
				alert(result.message || 'Connection test failed');
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
	}

	function getServerInfo(type: string): MCPServerInfo | undefined {
		return registry.find((r) => r.id === type);
	}

	function getConnectedCount(type: string): number {
		return integrations.filter((i) => i.serverType === type && i.connectionStatus === 'connected')
			.length;
	}
</script>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="font-display text-xl font-bold text-base-content mb-1">MCP Integrations</h2>
		<p class="text-sm text-base-content/60">Connect to external services via MCP</p>
	</div>
	<div class="flex gap-2">
		<Button type="ghost" onclick={loadIntegrations}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
		<Button type="primary" onclick={() => (showAddModal = true)}>
			<Plus class="w-4 h-4 mr-2" />
			Add Integration
		</Button>
	</div>
</div>

<!-- Available Integrations -->
<div class="grid sm:grid-cols-3 lg:grid-cols-4 gap-4 mb-8">
	{#each registry.filter((r) => !r.isBuiltin) as server}
		{@const connected = getConnectedCount(server.id)}
		<Card class="text-center">
			<div class="text-4xl mb-2">{server.icon || '🔌'}</div>
			<h3 class="font-display font-bold text-base-content">{server.name}</h3>
			<p class="text-xs text-base-content/60 mb-2">{server.description || ''}</p>
			<p class="text-sm text-base-content/60">
				{connected > 0 ? `${connected} connected` : 'Not connected'}
			</p>
		</Card>
	{/each}
</div>

<!-- Connected Integrations -->
<Card>
	<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
		<Link class="w-5 h-5" />
		Active Integrations
	</h2>

	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading integrations...</div>
	{:else if integrations.length === 0}
		<div class="py-12 text-center">
			<ExternalLink class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
			<h3 class="font-display font-bold text-base-content mb-2">No integrations configured</h3>
			<p class="text-base-content/60 mb-4">Connect to external services to extend capabilities</p>
			<Button type="secondary" onclick={() => (showAddModal = true)}>
				<Plus class="w-4 h-4 mr-2" />
				Add Integration
			</Button>
		</div>
	{:else}
		<div class="space-y-3">
			{#each integrations as integration}
				{@const serverInfo = getServerInfo(integration.serverType)}
				<div class="flex items-center justify-between p-4 rounded-lg bg-base-200">
					<div class="flex items-center gap-3">
						<div class="w-10 h-10 rounded-lg bg-secondary/10 flex items-center justify-center">
							<span class="text-xl">{serverInfo?.icon || '🔌'}</span>
						</div>
						<div>
							<div class="flex items-center gap-2">
								<span class="font-medium">{integration.name}</span>
								<span class="text-xs px-2 py-0.5 rounded bg-base-300"
									>{serverInfo?.name || integration.serverType}</span
								>
							</div>
							<div class="flex items-center gap-1 text-xs">
								{#if integration.connectionStatus === 'connected'}
									<CheckCircle class="w-3 h-3 text-success" />
									<span class="text-success">Connected</span>
								{:else if integration.connectionStatus === 'error'}
									<XCircle class="w-3 h-3 text-error" />
									<span class="text-error">Error</span>
								{:else}
									<XCircle class="w-3 h-3 text-base-content/40" />
									<span class="text-base-content/40">Disconnected</span>
								{/if}
								{#if integration.lastError}
									<span class="text-error/60 ml-2">· {integration.lastError}</span>
								{/if}
							</div>
						</div>
					</div>
					<div class="flex items-center gap-2">
						<Button type="ghost" size="sm" onclick={() => testIntegration(integration)}>
							<Play class="w-3 h-3 mr-1" />
							Test
						</Button>
						<Button type="ghost" size="sm" onclick={() => toggleIntegration(integration)}>
							{integration.isEnabled ? 'Disable' : 'Enable'}
						</Button>
						<button
							onclick={() => deleteIntegration(integration)}
							class="p-2 hover:bg-error/20 rounded text-error/60 hover:text-error"
						>
							<Trash2 class="w-4 h-4" />
						</button>
					</div>
				</div>
			{/each}
		</div>
	{/if}
</Card>

<!-- Built-in Integrations -->
{#if registry.filter((r) => r.isBuiltin).length > 0}
	{@const builtinServers = registry.filter((r) => r.isBuiltin)}
	<Card class="mt-6">
		<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
			<Server class="w-5 h-5" />
			Built-in Capabilities
		</h2>
		<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-3">
			{#each builtinServers as server}
				<div class="p-3 rounded-lg bg-base-200 flex items-center gap-3">
					<span class="text-2xl">{server.icon || '⚡'}</span>
					<div>
						<div class="font-medium text-sm">{server.name}</div>
						<div class="text-xs text-base-content/60">{server.description || 'Built-in'}</div>
					</div>
				</div>
			{/each}
		</div>
	</Card>
{/if}

<!-- Add Integration Modal -->
{#if showAddModal}
	{@const selectedServer = getServerInfo(selectedType)}
	<div
		class="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		aria-labelledby="add-mcp-title"
	>
		<button
			type="button"
			class="absolute inset-0 cursor-default"
			onclick={() => (showAddModal = false)}
			aria-label="Close modal"
		></button>
		<div class="bg-base-100 rounded-xl p-6 w-full max-w-md relative z-10">
			<h2 id="add-mcp-title" class="font-display text-xl font-bold mb-4">Add Integration</h2>

			<div class="space-y-4">
				<div>
					<label for="integration-type" class="block text-sm font-medium mb-1">Service</label>
					<select
						id="integration-type"
						bind:value={selectedType}
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					>
						{#each registry.filter((r) => !r.isBuiltin) as server}
							<option value={server.id}>{server.name}</option>
						{/each}
						<option value="custom">Custom MCP Server</option>
					</select>
				</div>

				{#if selectedServer?.description}
					<p class="text-sm text-base-content/60">{selectedServer.description}</p>
				{/if}

				<div>
					<label for="integration-name" class="block text-sm font-medium mb-1">Name</label>
					<input
						id="integration-name"
						type="text"
						bind:value={newIntegration.name}
						placeholder={selectedServer?.name || 'My Integration'}
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					/>
				</div>

				{#if selectedType === 'custom'}
					<div>
						<label for="integration-url" class="block text-sm font-medium mb-1">Server URL</label>
						<input
							id="integration-url"
							type="url"
							bind:value={newIntegration.serverUrl}
							placeholder="http://localhost:8080/mcp"
							class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
						/>
					</div>
				{/if}

				{#if selectedServer?.authType === 'api_key'}
					<div>
						<label for="integration-key" class="block text-sm font-medium mb-1">API Key</label>
						<input
							id="integration-key"
							type="password"
							bind:value={newIntegration.apiKey}
							placeholder={selectedServer?.apiKeyPlaceholder || 'Enter API key'}
							class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
						/>
						{#if selectedServer?.apiKeyUrl}
							<a
								href={selectedServer.apiKeyUrl}
								target="_blank"
								rel="noopener noreferrer"
								class="text-xs text-primary hover:underline mt-1 inline-block"
							>
								Get an API key →
							</a>
						{/if}
					</div>
				{/if}
			</div>

			<div class="flex gap-2 mt-6">
				<Button type="ghost" class="flex-1" onclick={() => (showAddModal = false)}> Cancel </Button>
				<Button type="primary" class="flex-1" onclick={addIntegration}> Add Integration </Button>
			</div>
		</div>
	</div>
{/if}
