<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { History, Trash2, MessageSquare, Clock, RefreshCw } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { AgentSession, SessionMessage } from '$lib/api/nebo';

	let sessions = $state<AgentSession[]>([]);
	let isLoading = $state(true);
	let selectedSession = $state<AgentSession | null>(null);
	let sessionMessages = $state<SessionMessage[]>([]);

	onMount(async () => {
		await loadSessions();
	});

	async function loadSessions() {
		isLoading = true;
		try {
			const data = await api.listAgentSessions();
			sessions = data.sessions || [];
		} catch (error) {
			console.error('Failed to load sessions:', error);
		} finally {
			isLoading = false;
		}
	}

	async function viewSession(session: AgentSession) {
		selectedSession = session;
		try {
			const data = await api.getAgentSessionMessages(session.id);
			sessionMessages = data.messages || [];
		} catch (error) {
			console.error('Failed to load messages:', error);
		}
	}

	async function deleteSession(session: AgentSession) {
		if (!confirm(`Delete session "${session.name || session.id}"?`)) return;
		try {
			await api.deleteAgentSession(session.id);
			sessions = sessions.filter(s => s.id !== session.id);
			if (selectedSession?.id === session.id) {
				selectedSession = null;
				sessionMessages = [];
			}
		} catch (error) {
			console.error('Failed to delete session:', error);
		}
	}

	function formatDate(dateStr: string): string {
		return new Date(dateStr).toLocaleString();
	}
</script>

<svelte:head>
	<title>Sessions - Nebo</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Sessions</h1>
		<p class="text-sm text-base-content/60">View and manage conversation history</p>
	</div>
	<Button type="ghost" onclick={loadSessions}>
		<RefreshCw class="w-4 h-4 mr-2" />
		Refresh
	</Button>
</div>

<div class="grid lg:grid-cols-3 gap-6">
	<!-- Sessions List -->
	<div class="lg:col-span-1">
		<Card>
			<h2 class="font-display font-bold text-base-content mb-4 flex items-center gap-2">
				<History class="w-5 h-5" />
				All Sessions
			</h2>
			{#if isLoading}
				<div class="py-8 text-center text-base-content/60">Loading...</div>
			{:else if sessions.length === 0}
				<div class="py-8 text-center text-base-content/60">
					<MessageSquare class="w-8 h-8 mx-auto mb-2 opacity-50" />
					<p>No sessions yet</p>
				</div>
			{:else}
				<div class="space-y-2">
					{#each sessions as session}
						<div
							class="w-full p-3 rounded-lg text-left transition-colors cursor-pointer {selectedSession?.id === session.id ? 'bg-primary/10 border border-primary/30' : 'bg-base-200 hover:bg-base-300'}"
							onclick={() => viewSession(session)}
							onkeydown={(e) => e.key === 'Enter' && viewSession(session)}
							role="button"
							tabindex="0"
						>
							<div class="flex items-center justify-between mb-1">
								<span class="font-medium text-sm truncate">{session.name || session.id}</span>
								<button
									onclick={(e) => { e.stopPropagation(); deleteSession(session); }}
									class="p-1 hover:bg-error/20 rounded text-error/60 hover:text-error"
								>
									<Trash2 class="w-3 h-3" />
								</button>
							</div>
							<div class="flex items-center gap-3 text-xs text-base-content/50">
								<span class="flex items-center gap-1">
									<MessageSquare class="w-3 h-3" />
									{session.messageCount} messages
								</span>
								<span class="flex items-center gap-1">
									<Clock class="w-3 h-3" />
									{formatDate(session.updatedAt)}
								</span>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</Card>
	</div>

	<!-- Session Messages -->
	<div class="lg:col-span-2">
		<Card class="h-[calc(100vh-220px)]">
			{#if selectedSession}
				<h2 class="font-display font-bold text-base-content mb-4">
					{selectedSession.name || selectedSession.id}
				</h2>
				<div class="overflow-y-auto h-[calc(100%-3rem)] space-y-3">
					{#if sessionMessages.length === 0}
						<div class="py-8 text-center text-base-content/60">No messages in this session</div>
					{:else}
						{#each sessionMessages as msg}
							<div class="p-3 rounded-lg {msg.role === 'user' ? 'bg-primary/10' : 'bg-base-200'}">
								<div class="flex items-center gap-2 mb-1">
									<span class="text-xs font-medium uppercase {msg.role === 'user' ? 'text-primary' : 'text-secondary'}">
										{msg.role}
									</span>
									<span class="text-xs text-base-content/40">
										{formatDate(msg.createdAt)}
									</span>
								</div>
								<p class="text-sm whitespace-pre-wrap">{msg.content}</p>
							</div>
						{/each}
					{/if}
				</div>
			{:else}
				<div class="h-full flex items-center justify-center text-center">
					<div>
						<History class="w-12 h-12 mx-auto mb-4 text-base-content/30" />
						<p class="text-base-content/60">Select a session to view messages</p>
					</div>
				</div>
			{/if}
		</Card>
	</div>
</div>
