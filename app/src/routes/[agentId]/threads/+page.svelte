<script lang="ts">
  import { getContext } from 'svelte';
  import { goto } from '$app/navigation';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);

  async function handleSend(text: string) {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.createNewAgentChat(agentId);
      const newChatId = (resp as Record<string, any>)?.chat?.id;
      if (!newChatId) return;

      const { getWebSocketClient } = await import('$lib/websocket/client');
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('chat', { prompt: text, agent_id: agentId });
      } else {
        await api.chatWithAgent(agentId, { prompt: text });
      }

      goto(`/${agentId}/threads/${newChatId}`);
    } catch (e) {
      console.warn('[nebo] Failed to create thread', e);
    }
  }
</script>

<ChatPane
  messages={[]}
  agentName={agent?.name ?? 'Agent'}
  agentId={agentId}
  headerTitle="New thread"
  headerRight="Creations"
  placeholder="Start a new thread with {agent?.name}..."
  emptyIcon={agent?.initial}
  emptyTitle={agent?.name}
  emptyDesc="New thread · clean context. {agent?.name} knows who you are but nothing about previous threads."
  onsend={handleSend}
/>
