<script lang="ts">
  import { launchApp } from '$lib/apps/launcher';
  import FlowsPane from '$lib/components/flows/FlowsPane.svelte';
  import { getContext, onDestroy, onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import type { Agent } from '$lib/api/neboComponents';
  import { currentUser } from '$lib/stores/auth';
  import { sendInstallCode } from '$lib/marketplace/installCodes';
  import { createChatController } from '$lib/chat/controller.svelte';
  import { toMentionAgent } from '$lib/chat/roster';
  import { threadIdFromKey, threadKey } from '$lib/chat/sessionKey';

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

  // New-thread page: the controller owns chat state + WS wiring (quota warnings,
  // chat errors, roster). The run itself starts on the thread page after
  // navigation (see handleSend's pending-send stash), so no real session exists
  // yet — the placeholder thread key keeps session-tagged streams from this
  // agent's OTHER threads off the empty page, while untagged quota warnings and
  // errors still surface.
  const chat = createChatController({ agentId: ctx.agentId, sessionKey: threadKey(ctx.agentId, 'pending') });
  onDestroy(() => chat.destroy());

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        chat.setAllAgents((resp.agents).map(toMentionAgent));
      }
    } catch { /* keep empty */ }
  });

  async function handleSend(text: string) {
    // Detect marketplace code — the install modal owns all feedback, so open it
    // immediately and skip the chat "working" spinner (no agent reply is coming).
    if (sendInstallCode(text, agentId)) return;

    chat.isLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.createNewAgentChat(agentId);
      const newChatId = (resp as Record<string, any>)?.chat?.id;
      if (!newChatId) {
        chat.isLoading = false;
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
      chat.isLoading = false;
    }
  }

  // ?ask= — a starter prompt from a pane CTA lands in the composer, then the
  // param is cleared so refresh doesn't re-insert it.
  const askPrefill = $derived($page.url.searchParams.get('ask') ?? '');
  function clearAsk() {
    const url = new URL($page.url);
    url.searchParams.delete('ask');
    goto(url.pathname + url.search, { replaceState: true, noScroll: true, keepFocus: true });
  }
</script>

<ChatPane
  messages={chat.messages}
  agentName={agent?.name ?? $t('common.agent')}
  agentId={agentId}
  headerTitle={$t('chat.newThread')}
  headerRight={$t('chat.work')}
  onopenruns={ctx.openRuns}
  composerPrefill={askPrefill}
  onprefilled={clearAsk}
  onback={ctx.openList}
  onsettings={ctx.openSettings}
  isolated={ctx.agent?.isolated ?? false}
  isApp={ctx.agent?.isApp ?? false}
  onopenapp={() => launchApp(ctx.agentId, ctx.agent?.name ?? 'App')}

  placeholder={$t('chat.startNewThreadWith', { values: { name: agent?.name ?? '' } })}
  emptyTitle={greeting}
  emptyDesc={$t('chat.newThreadEmptyDesc', { values: { name: agent?.name ?? $t('chat.yourEmployee') } })}
  allAgents={chat.allAgents}
  onsend={handleSend}
  onteachsent={(_message, sessionKey) => {
    const chatId = threadIdFromKey(sessionKey);
    if (chatId) goto(`/${agentId}/threads/${chatId}?active=1`);
  }}
  isLoading={chat.isLoading}
  quotaWarning={chat.quotaWarning}
  ondismisswarning={() => chat.dismissWarning()}
  chatError={chat.chatError}
  ondismisserror={() => chat.dismissError()}
>
  {#snippet flowsPane()}<FlowsPane onask={ctx.askEmployee} />{/snippet}
</ChatPane>
