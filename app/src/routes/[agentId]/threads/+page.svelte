<script lang="ts">
  import { getContext, onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import { currentUser } from '$lib/stores/auth';

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

  let cleanupQuotaWarning: (() => void) | null = null;

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

    function onQuotaWarning(e: Event) {
      const data = (e as CustomEvent).detail;
      if (data?.text) quotaWarning = data.text;
    }
    window.addEventListener('nebo:quota_warning', onQuotaWarning);
    cleanupQuotaWarning = () => window.removeEventListener('nebo:quota_warning', onQuotaWarning);
  });

  onDestroy(() => {
    cleanupQuotaWarning?.();
  });

  // Marketplace code pattern for instant modal feedback
  const CODE_RE = /^(NEBO|SKIL|WORK|AGNT|LOOP|PLUG|APPX)-[0-9A-Z]{4}-[0-9A-Z]{4}$/i;
  const CODE_TYPE_MAP: Record<string, string> = {
    NEBO: 'nebo', SKIL: 'skill', WORK: 'workflow', AGNT: 'agent',
    LOOP: 'loop', PLUG: 'plugin', APPX: 'app',
  };
  const CODE_STATUS_MAP: Record<string, string> = {
    nebo: 'Connecting to NeboLoop...', skill: 'Installing skill...',
    workflow: 'Installing workflow...', agent: 'Installing agent...',
    loop: 'Joining loop...', plugin: 'Installing plugin...', app: 'Installing app...',
  };

  async function handleSend(text: string) {
    // Detect marketplace code — show install modal immediately
    const codeMatch = text.trim().match(CODE_RE);
    if (codeMatch) {
      const prefix = codeMatch[1].toUpperCase();
      const codeTypeStr = CODE_TYPE_MAP[prefix] || 'code';
      window.dispatchEvent(new CustomEvent('nebo:code_processing', {
        detail: {
          code: text.trim().toUpperCase(),
          code_type: codeTypeStr,
          status_message: CODE_STATUS_MAP[codeTypeStr] || 'Processing...',
        },
      }));
    }

    messages = [{ id: 'msg-' + Date.now(), type: 'user' as const, content: text, time: 'now' }];
    isLoading = true;
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
  headerRight="Creations"
  placeholder="Start a new thread with {agent?.name}..."
  emptyTitle={greeting}
  emptyDesc="New thread with {agent?.name ?? 'your companion'} · clean context, fresh start."
  {allAgents}
  onsend={handleSend}
  {isLoading}
  {quotaWarning}
  ondismisswarning={() => quotaWarning = undefined}
/>
