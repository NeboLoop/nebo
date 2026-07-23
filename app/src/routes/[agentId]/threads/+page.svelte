<script lang="ts">
  import { getContext, onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { goto } from '$lib/nav';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import { currentUser } from '$lib/stores/auth';
  import { dispatchInstallStart } from '$lib/marketplace/installCodes';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);

  // Returns an i18n key — translated in the derived below.
  function getGreeting(): string {
    const hour = new Date().getHours();
    if (hour < 12) return 'chat.goodMorning';
    if (hour < 17) return 'chat.goodAfternoon';
    return 'chat.goodEvening';
  }

  const firstName = $derived($currentUser?.name?.split(' ')[0] ?? '');
  const greeting = $derived(firstName
    ? $t('chat.greetingWithName', { values: { greeting: $t(getGreeting()), name: firstName } })
    : $t(getGreeting()));

  let messages = $state<any[]>([]);
  let isLoading = $state(false);
  let allAgents = $state<{ id: string; name: string; role: string; initial: string; status: string; color: string }[]>([]);
  let quotaWarning = $state<string | undefined>(undefined);

  let chatError = $state<string | undefined>(undefined);

  // Quota warnings: idiomatic WS subscription, auto-cleaned up on destroy.
  onWsEvent<{ text?: string }>('quota_warning', (d) => {
    if (d?.text) quotaWarning = d.text;
  });

  onWsEvent<{ error?: string }>('chat_error', (d) => {
    if (d?.error) { chatError = d.error; isLoading = false; }
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

    isLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.createNewAgentChat(agentId);
      const newChatId = (resp as Record<string, any>)?.chat?.id;
      if (!newChatId) {
        isLoading = false;
        return;
      }

      // Create the thread, then navigate — the thread page sends the prompt after
      // its chat controller is subscribed. Sending here raced navigation and the
      // optimistic bubble (and often the whole turn) disappeared on the new page.
      sessionStorage.setItem(
        `nebo:pending-send:${newChatId}`,
        JSON.stringify({ text, ts: Date.now() }),
      );
      goto(`/${agentId}/threads/${newChatId}?active=1`);
    } catch (e) {
      console.warn('[nebo] Failed to create thread', e);
      isLoading = false;
    }
  }
</script>

<ChatPane
  {messages}
  agentName={agent?.name ?? $t('common.agent')}
  agentId={agentId}
  headerTitle={$t('chat.newThread')}
  headerRight={$t('chat.work')}
  placeholder={$t('chat.startNewThreadWith', { values: { name: agent?.name ?? '' } })}
  emptyTitle={greeting}
  emptyDesc={$t('chat.newThreadEmptyDesc', { values: { name: agent?.name ?? $t('chat.yourEmployee') } })}
  {allAgents}
  onsend={handleSend}
  {isLoading}
  {quotaWarning}
  ondismisswarning={() => quotaWarning = undefined}
  {chatError}
  ondismisserror={() => chatError = undefined}
/>
