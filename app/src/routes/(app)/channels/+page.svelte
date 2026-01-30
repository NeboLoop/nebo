<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { MessageCircle, Plus, Settings, Trash2, CheckCircle, XCircle, RefreshCw } from 'lucide-svelte';

	interface Channel {
		id: string;
		type: 'telegram' | 'discord' | 'slack';
		name: string;
		status: 'connected' | 'disconnected' | 'error';
		config: Record<string, string>;
	}

	let channels = $state<Channel[]>([]);
	let isLoading = $state(true);
	let showAddModal = $state(false);
	let newChannel = $state({ type: 'telegram', name: '', token: '' });

	const channelInfo = {
		telegram: {
			name: 'Telegram',
			icon: '📱',
			color: 'bg-blue-500/10 text-blue-500',
			fields: ['Bot Token']
		},
		discord: {
			name: 'Discord',
			icon: '🎮',
			color: 'bg-indigo-500/10 text-indigo-500',
			fields: ['Bot Token', 'Guild ID']
		},
		slack: {
			name: 'Slack',
			icon: '💼',
			color: 'bg-purple-500/10 text-purple-500',
			fields: ['Bot Token', 'Signing Secret']
		}
	};

	onMount(async () => {
		await loadChannels();
	});

	async function loadChannels() {
		isLoading = true;
		try {
			const response = await fetch('/api/v1/agent/channels');
			if (response.ok) {
				const data = await response.json();
				channels = data.channels || [];
			}
		} catch (error) {
			console.error('Failed to load channels:', error);
		} finally {
			isLoading = false;
		}
	}

	async function addChannel() {
		try {
			const response = await fetch('/api/v1/agent/channels', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(newChannel)
			});
			if (response.ok) {
				await loadChannels();
				showAddModal = false;
				newChannel = { type: 'telegram', name: '', token: '' };
			}
		} catch (error) {
			console.error('Failed to add channel:', error);
		}
	}

	async function deleteChannel(channel: Channel) {
		if (!confirm(`Delete ${channel.name}?`)) return;
		try {
			const response = await fetch(`/api/v1/agent/channels/${channel.id}`, {
				method: 'DELETE'
			});
			if (response.ok) {
				channels = channels.filter(c => c.id !== channel.id);
			}
		} catch (error) {
			console.error('Failed to delete channel:', error);
		}
	}

	async function toggleChannel(channel: Channel) {
		const action = channel.status === 'connected' ? 'disconnect' : 'connect';
		try {
			const response = await fetch(`/api/v1/agent/channels/${channel.id}/${action}`, {
				method: 'POST'
			});
			if (response.ok) {
				await loadChannels();
			}
		} catch (error) {
			console.error(`Failed to ${action} channel:`, error);
		}
	}
</script>

<svelte:head>
	<title>Channels - Nebo</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Channels</h1>
		<p class="text-sm text-base-content/60">Connect messaging platforms to your agent</p>
	</div>
	<div class="flex gap-2">
		<Button type="ghost" onclick={loadChannels}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
		<Button type="primary" onclick={() => showAddModal = true}>
			<Plus class="w-4 h-4 mr-2" />
			Add Channel
		</Button>
	</div>
</div>

<!-- Channel Types Overview -->
<div class="grid sm:grid-cols-3 gap-4 mb-8">
	{#each Object.entries(channelInfo) as [type, info]}
		{@const connected = channels.filter(c => c.type === type && c.status === 'connected').length}
		<Card class="text-center">
			<div class="text-4xl mb-2">{info.icon}</div>
			<h3 class="font-display font-bold text-base-content">{info.name}</h3>
			<p class="text-sm text-base-content/60">
				{connected} connected
			</p>
		</Card>
	{/each}
</div>

<!-- Connected Channels -->
<Card>
	<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
		<MessageCircle class="w-5 h-5" />
		Connected Channels
	</h2>

	{#if isLoading}
		<div class="py-8 text-center text-base-content/60">Loading channels...</div>
	{:else if channels.length === 0}
		<div class="py-12 text-center">
			<MessageCircle class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
			<h3 class="font-display font-bold text-base-content mb-2">No channels configured</h3>
			<p class="text-base-content/60 mb-4">Add a channel to start receiving messages</p>
			<Button type="primary" onclick={() => showAddModal = true}>
				<Plus class="w-4 h-4 mr-2" />
				Add Your First Channel
			</Button>
		</div>
	{:else}
		<div class="space-y-3">
			{#each channels as channel}
				{@const info = channelInfo[channel.type]}
				<div class="flex items-center justify-between p-4 rounded-lg bg-base-200">
					<div class="flex items-center gap-3">
						<div class="w-10 h-10 rounded-lg {info.color} flex items-center justify-center text-xl">
							{info.icon}
						</div>
						<div>
							<div class="flex items-center gap-2">
								<span class="font-medium">{channel.name}</span>
								<span class="text-xs px-2 py-0.5 rounded bg-base-300">{info.name}</span>
							</div>
							<div class="flex items-center gap-1 text-xs">
								{#if channel.status === 'connected'}
									<CheckCircle class="w-3 h-3 text-success" />
									<span class="text-success">Connected</span>
								{:else if channel.status === 'error'}
									<XCircle class="w-3 h-3 text-error" />
									<span class="text-error">Error</span>
								{:else}
									<XCircle class="w-3 h-3 text-base-content/40" />
									<span class="text-base-content/40">Disconnected</span>
								{/if}
							</div>
						</div>
					</div>
					<div class="flex items-center gap-2">
						<Button type="ghost" size="sm" onclick={() => toggleChannel(channel)}>
							{channel.status === 'connected' ? 'Disconnect' : 'Connect'}
						</Button>
						<button
							onclick={() => deleteChannel(channel)}
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

<!-- Add Channel Modal -->
{#if showAddModal}
	<div
		class="fixed inset-0 bg-black/50 flex items-center justify-center z-50"
		role="dialog"
		aria-modal="true"
		aria-labelledby="add-channel-title"
	>
		<button
			type="button"
			class="absolute inset-0 cursor-default"
			onclick={() => showAddModal = false}
			aria-label="Close modal"
		></button>
		<div class="bg-base-100 rounded-xl p-6 w-full max-w-md relative z-10">
			<h2 id="add-channel-title" class="font-display text-xl font-bold mb-4">Add Channel</h2>

			<div class="space-y-4">
				<div>
					<label for="channel-platform" class="block text-sm font-medium mb-1">Platform</label>
					<select
						id="channel-platform"
						bind:value={newChannel.type}
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					>
						<option value="telegram">Telegram</option>
						<option value="discord">Discord</option>
						<option value="slack">Slack</option>
					</select>
				</div>

				<div>
					<label for="channel-name" class="block text-sm font-medium mb-1">Name</label>
					<input
						id="channel-name"
						type="text"
						bind:value={newChannel.name}
						placeholder="My Bot"
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					/>
				</div>

				<div>
					<label for="channel-token" class="block text-sm font-medium mb-1">Bot Token</label>
					<input
						id="channel-token"
						type="password"
						bind:value={newChannel.token}
						placeholder="Enter bot token"
						class="w-full px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50"
					/>
				</div>
			</div>

			<div class="flex gap-2 mt-6">
				<Button type="ghost" class="flex-1" onclick={() => showAddModal = false}>
					Cancel
				</Button>
				<Button type="primary" class="flex-1" onclick={addChannel}>
					Add Channel
				</Button>
			</div>
		</div>
	</div>
{/if}
