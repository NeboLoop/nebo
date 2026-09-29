<!--
  TeamWorkSheet — what one member (or one helper a member started) is doing
  in a team's conversation, opened from the team's working strip: its live
  activity and its steps and tool calls so far, read from the chat that
  holds them and kept live by the same events a chat is, with Stop and
  Redirect at the bottom. The sheet is the app's ShelfModal (a bottom sheet
  on phones; swipe down or tap outside to close).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, untrack } from 'svelte';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import { createChatController } from '$lib/chat/controller.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { teamWorking } from '$lib/api/nebo';
  import type { WorkEntry } from '$lib/teams/teamWork';

  let {
    teamId,
    entry,
    live,
    onclose,
    onstop,
    onredirect,
  }: {
    teamId: string;
    entry: WorkEntry;
    /** Still at work: once it stops, the sheet shows what it did. */
    live: boolean;
    onclose: () => void;
    onstop: (entry: WorkEntry) => void;
    onredirect: (entry: WorkEntry) => void;
  } = $props();

  // The chat is read, never written: the controller is only the live view.
  // The sheet is keyed per entry, so the entry it watches never changes.
  const chat = untrack(() => createChatController({ agentId: entry.agentId, sessionKey: entry.sessionKey }));
  let chatId = '';

  onMount(() => {
    (async () => {
      chatId = entry.chatId;
      if (!chatId) {
        // Seen first as an event: the chat that holds its steps is named
        // by the team's working list.
        const resp = await teamWorking(teamId).catch(() => null);
        chatId = resp?.working.find((w) => w.sessionKey === entry.sessionKey)?.chatId ?? '';
      }
      if (chatId) await chat.loadHistory(chatId);
    })();
    // A helper's steps are not streamed to the app; each progress line it
    // sends is the moment its stored steps changed.
    const off = getWebSocketClient().on('subagent_progress', (data: any) => {
      if (entry.kind === 'helper' && data?.task_id === entry.taskId && chatId) chat.loadHistory(chatId);
    });
    return () => {
      off();
      chat.destroy();
    };
  });
</script>

<ShelfModal
  open={true}
  title={entry.title}
  subtitle={entry.kind === 'helper' ? $t('teams.helperOf', { values: { name: entry.member } }) : entry.activity || $t('sidebar.working')}
  {onclose}
>
  <div class="flex-1 min-w-0 min-h-0 flex flex-col">
    <ChatPane
      messages={chat.messages}
      agentName={entry.member}
      agentId={entry.agentId}
      sessionId={entry.sessionKey}
      isLoading={live}
      activityStatus={entry.activity || chat.activityStatus}
      historyLoading={chat.historyLoading}
      readOnly={true}
    />
    <div class="shrink-0 border-t border-base-300 px-5 py-3 flex items-center justify-end gap-2 max-md:pb-[max(0.75rem,env(safe-area-inset-bottom))]">
      <button type="button" class="btn btn-ghost btn-sm" onclick={() => onredirect(entry)}>{$t('teams.redirect')}</button>
      {#if live}
        <button type="button" class="btn btn-error btn-sm" onclick={() => onstop(entry)}>{$t('common.stop')}</button>
      {/if}
    </div>
  </div>
</ShelfModal>
