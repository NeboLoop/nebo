<script lang="ts">
  import { getContext, onMount } from 'svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { goto } from '$app/navigation';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import { currentUser } from '$lib/stores/auth';
  import { dispatchInstallStart } from '$lib/marketplace/installCodes';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);

  function getGreeting(): string {
    const hour = new Date().getHours();
    if (hour < 12) return 'Good morning';
    if (hour < 17) return 'Good afternoon';
    return 'Good evening';
  }

  const firstName = $derived($currentUser?.name?.split(' ')[0] ?? '');
  const greeting = $derived(firstName ? `${getGreeting()}, ${firstName}` : getGreeting());

  let messages = $state<any[]>([]);
  let isLoading = $state(false);
  let allAgents = $state<{ id: string; name: string; role: string; initial: string; status: string; color: string }[]>([]);
  let quotaWarning = $state<string | undefined>(undefined);

  // Quota warnings: idiomatic WS subscription, auto-cleaned up on destroy.
  onWsEvent<{ text?: string }>('quota_warning', (d) => {
    if (d?.text) quotaWarning = d.text;
  });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        allAgents = resp.agents.map((a: any) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        }));
      }
    } catch { /* keep empty */ }
  });

  async function handleSend(text: string) {
    // Detect marketplace code — the install modal owns all feedback, so open it
    // immediately and skip the chat "working" spinner (no agent reply is coming).
    if (dispatchInstallStart(text)) {
      const { getWebSocketClient } = await import('$lib/websocket/client');
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('chat', { prompt: text.trim(), agent_id: agentId });
      } else {
        const api = await import('$lib/api/nebo');
        await api.chatWithAgent(agentId, { prompt: text.trim() });
      }
      return;
    }

    messages = [{ id: 'msg-' + Date.now(), type: 'user' as const, content: text, time: 'now' }];
    isLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.createNewAgentChat(agentId);
      const newChatId = (resp as Record<string, any>)?.chat?.id;
      if (!newChatId) return;

      const sessionKey = `agent:${agentId}:thread:${newChatId}`;
      console.log('[THREAD-DEBUG] new thread send:', { sessionKey, newChatId, agentId });
      const { getWebSocketClient } = await import('$lib/websocket/client');
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('chat', { prompt: text, agent_id: agentId, session_id: sessionKey });
      } else {
        await api.chatWithAgent(agentId, { prompt: text });
      }

      goto(`/${agentId}/threads/${newChatId}?active=1`);
    } catch (e) {
      console.warn('[nebo] Failed to create thread', e);
      isLoading = false;
    }
  }
</script>

<ChatPane
  {messages}
  agentName={agent?.name ?? 'Agent'}
  agentId={agentId}
  headerTitle="New thread"
  headerRight="Work"
  placeholder="Start a new thread with {agent?.name}..."
  emptyTitle={greeting}
  emptyDesc="New thread with {agent?.name ?? 'your companion'} · clean context, fresh start."
  {allAgents}
  onsend={handleSend}
  {isLoading}
  {quotaWarning}
  ondismisswarning={() => quotaWarning = undefined}
/>
