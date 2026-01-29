<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Server, Plus, Trash2, CheckCircle, XCircle, RefreshCw, Link, ExternalLink } from 'lucide-svelte';

	interface MCPConnection {
		id: string;
		name: string;
		url: string;
		status: 'connected' | 'disconnected' | 'error';
		tools_count: number;
		last_ping?: string;
	}

	let connections = $state<MCPConnection[]>([]);
	let isLoading = $state(true);
	let showAddModal = $state(false);
	let newConnection = $state({ name: '', url: '' });

	// Local MCP server status
	let localMCP = $state({
		url: 'http://localhost:27895/mcp',
		status: 'unknown' as 'connected' | 'disconnected' | 'unknown',
		tools_count: 0
	});

	onMount(async () => {
		await Promise.all([checkLocalMCP(), loadConnections()]);
	});

	async function checkLocalMCP() {
		try {
			const response = await fetch('/api/v1/agent/mcp/status');
			if (response.ok) {
				const data = await response.json();
				localMCP = {
					url: data.url || 'http://localhost:27895/mcp',
					status: data.status || 'connected',
					tools_count: data.tools_count || 0
				};
			}
		} catch (error) {
			localMCP.status = 'disconnected';
		}
	}

	async function loadConnections() {
		isLoading = true;
		try {
			const response = await fetch('/api/v1/agent/mcp/connections');
			if (response.ok) {
				const data = await response.json();
				connections = data.connections || [];
			}
		} catch (error) {
			console.error('Failed to load connections:', error);
		} finally {
			isLoading = false;
		}
	}

	async function addConnection() {
		try {
			const response = await fetch('/api/v1/agent/mcp/connections', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(newConnection)
			});
			if (response.ok) {
				await loadConnections();
				showAddModal = false;
				newConnection = { name: '', url: '' };
			}
		} catch (error) {
			console.error('Failed to add connection:', error);
		}
	}

	async function deleteConnection(conn: MCPConnection) {
		if (!confirm(`Remove ${conn.name}?`)) return;
		try {
			const response = await fetch(`/api/v1/agent/mcp/connections/${conn.id}`, {
				method: 'DELETE'
			});
			if (response.ok) {
				connections = connections.filter(c => c.id !== conn.id);
			}
		} catch (error) {
			console.error('Failed to delete connection:', error);
		}
	}

	async function testConnection(conn: MCPConnection) {
		try {
			const response = await fetch(`/api/v1/agent/mcp/connections/${conn.id}/test`, {
				method: 'POST'
			});
			if (response.ok) {
				await loadConnections();
			}
		} catch (error) {
			console.error('Failed to test connection:', error);
		}
	}
</script>

<svelte:head>
	<title>MCP Connections - GoBot</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">MCP Connections</h1>
		<p class="text-sm text-base-content/60">Connect to Model Context Protocol servers</p>
	</div>
	<div class="flex gap-2">
		<Button type="ghost" onclick={() => { checkLocalMCP(); loadConnections(); }}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
		<Button type="primary" onclick={() => showAddModal = true}>
			<Plus class="w-4 h-4 mr-2" />
			Add Connection
		</Button>
	</div>
</div>

<!-- Local MCP Status -->
<Card class="mb-6">
	<div class="flex items-center justify-between">
		<div class="flex items-center gap-4">
			<div class="w-12 h-12 rounded-xl bg-primary/10 flex items-center justify-center">
				<Server class="w-6 h-6 text-primary" />
			</div>
			<div>
				<h2 class="font-display font-bold text-base-content">Local MCP Server</h2>
				<p class="text-sm text-base-content/60">{localMCP.url}</p>
			</div>
		</div>
		<div class="flex items-center gap-4">
			<div class="text-right">
				<div class="flex items-center gap-2">
					{#if localMCP.status === 'connected'}
						<CheckCircle class="w-4 h-4 text-success" />
						<span class="text-success font-medium">Connected</span>
					{:else}
						<XCircle class="w-4 h-4 text-error" />
						<span class="text-error font-medium">Disconnected</span>
					{/if}
				</div>
				{#if localMCP.tools_count > 0}
					<p class="text-xs text-base-content/50">{localMCP.tools_count} tools available</p>
				{/if}
			</div>
		</div>
	</div>
</Card>

<!-- External Connections -->
<Card>
	<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
		<Link class="w-5 h-5" />
		External MCP Servers
	</h2>

	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading connections...</div>
	{:else if connections.length === 0}
		<div class="py-12 text-center">
			<ExternalLink class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
			<h3 class="font-display font-bold text-base-content mb-2">No external connections</h3>
			<p class="text-base-content/60 mb-4">Connect to external MCP servers to extend capabilities</p>
			<Button type="secondary" onclick={() => showAddModal = true}>
				<Plus class="w-4 h-4 mr-2" />
				Add Connection
			</Button>
		</div>
	{:else}
		<div class="space-y-3">
			{#each connections as conn}
				<div class="flex items-center justify-between p-4 rounded-lg bg-base-200">
					<div class="flex items-center gap-3">
						<div class="w-10 h-10 rounded-lg bg-secondary/10 flex items-center justify-center">
							<Server class="w-5 h-5 text-secondary" />
						</div>
						<div>
							<div class="font-medium">{conn.name}</div>
							<div class="text-xs text-base-content/50">{conn.url}</div>
						</div>
					</div>
					<div class="flex items-center gap-3">
						<div class="text-right text-sm">
							{#if conn.status === 'connected'}
								<span class="text-success flex items-center gap-1">
									<CheckCircle class="w-3 h-3" /> Connected
								</span>
							{:else if conn.status === 'error'}
								<span class="text-error flex items-center gap-1">
									<XCircle class="w-3 h-3" /> Error
								</span>
							{:else}
								<span class="text-base-content/40 flex items-center gap-1">
									<XCircle class="w-3 h-3" /> Disconnected
								</span>
							{/if}
							{#if conn.tools_count > 0}
								<div class="text-xs text-base-content/40">{conn.tools_count} tools</div>
							{/if}
						</div>
						<Button type="ghost" size="sm" onclick={() => testConnection(conn)}>
							Test
						</Button>
						<button
							onclick={() => deleteConnection(conn)}
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

<!-- Add Connection Modal -->
{#if showAddModal}
	<div
		class="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		aria-labelledby="add-mcp-title"
	>
		<button
			type="button"
			class="absolute inset-0 cursor-default"
			onclick={() => showAddModal = false}
			aria-label="Close modal"
		></button>
		<div class="bg-base-100 rounded-xl p-6 w-full max-w-md relative z-10">
			<h2 id="add-mcp-title" class="font-display text-xl font-bold mb-4">Add MCP Connection</h2>

			<div class="space-y-4">
				<div>
					<label for="mcp-name" class="block text-sm font-medium mb-1">Name</label>
					<input
						id="mcp-name"
						type="text"
						bind:value={newConnection.name}
						placeholder="My MCP Server"
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					/>
				</div>

				<div>
					<label for="mcp-url" class="block text-sm font-medium mb-1">Server URL</label>
					<input
						id="mcp-url"
						type="url"
						bind:value={newConnection.url}
						placeholder="http://localhost:8080/mcp"
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					/>
				</div>
			</div>

			<div class="flex gap-2 mt-6">
				<Button type="ghost" class="flex-1" onclick={() => showAddModal = false}>
					Cancel
				</Button>
				<Button type="primary" class="flex-1" onclick={addConnection}>
					Add Connection
				</Button>
			</div>
		</div>
	</div>
{/if}
