<!--
  A piece of background work's transcript: the chat that holds its own
  steps (a helper's, a coworker's conversation for a hand-off, a turn's or a
  workflow run's), read-only, kept live while the work runs. Opened from
  "View transcript" on a row. The app's ShelfModal (a bottom sheet on
  phones; swipe down or tap outside to close).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, untrack } from 'svelte';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import { createChatController } from '$lib/chat/controller.svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { callWords } from '$lib/chat/callLabel';
  import type { BackgroundTask } from '$lib/api/neboComponents';
  import { backgroundTasks, kindLabel } from '$lib/stores/background';

  let { task, onclose }: { task: BackgroundTask; onclose: () => void } = $props();

  // The chat is read, never written: the controller is only the live view.
  // The sheet is keyed per task, so the chat it watches never changes.
  const chat = untrack(() => createChatController({ agentId: task.agentId, sessionKey: task.transcript?.sessionKey }));
  const chatId = untrack(() => task.transcript?.chatId ?? '');

  /** The row as it is now; gone once the work ended. */
  const live = $derived($backgroundTasks.find((x) => x.id === task.id) ?? null);
  const doing = $derived(live?.call ? callWords(live.call, $t) : (live?.detail ?? ''));

  onMount(() => {
    if (chatId) void chat.loadHistory(chatId);
    // A helper's steps are not streamed to the app; each progress line it
    // sends is the moment its stored steps changed.
    const ws = getWebSocketClient();
    const reread = () => {
      if (chatId) void chat.loadHistory(chatId);
    };
    const offs = [ws.on('subagent_progress', reread), ws.on('background_changed', reread)];
    return () => {
      offs.forEach((off) => off());
      chat.destroy();
    };
  });
</script>

<ShelfModal open={true} title={task.title || task.employee} subtitle={kindLabel(task, $t)} {onclose}>
  <div class="flex-1 min-w-0 min-h-0 flex flex-col">
    <ChatPane
      messages={chat.messages}
      agentName={task.employee}
      agentId={task.agentId}
      isLoading={live !== null}
      activityStatus={doing || chat.activityStatus}
      historyLoading={chat.historyLoading}
      readOnly={true}
    />
  </div>
</ShelfModal>
