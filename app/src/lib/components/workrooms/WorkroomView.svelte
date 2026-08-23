<!--
  WorkroomView — a mission room. The room IS a loop channel; the hub owns the
  conversation, this view is the owner's seat in it. Unlike the coworker
  transcript, the owner's composer is LIVE here: a room is a shared space the
  owner participates in — type the mission, @-name employees, redirect
  mid-flight.

  Data: initial load from GET /workrooms/{id}/messages, then `workroom_message`
  WS events (never polling). Employees answer when addressed — channel
  dispatch is mention-driven, which is the room's addressed-only rule.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, tick } from 'svelte';
  import { getWorkroomMessages, sendWorkroomMessage } from '$lib/api/nebo';
  import type { Workroom, WorkroomMessage } from '$lib/api/neboComponents';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { parseMarkdown } from '$lib/markdown';

  let { room }: { room: Workroom } = $props();

  type RoomMsg = {
    id: string;
    from: string;
    content: string;
    mine: boolean;
  };

  let messages: RoomMsg[] = $state([]);
  let loading = $state(true);
  let draft = $state('');
  let sending = $state(false);
  let scroller = $state<HTMLDivElement | null>(null);

  // The hub's history rows carry `role` for human-injected legs; live WS
  // events carry senderName. Both normalize to the same shape.
  const fromRest = (m: WorkroomMessage): RoomMsg => ({
    id: m.id,
    from: m.from,
    content: m.content,
    mine: m.role === 'user',
  });

  async function scrollToEnd() {
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight });
  }

  onMount(() => {
    (async () => {
      try {
        const resp = await getWorkroomMessages(room.channelId);
        messages = (resp?.messages ?? []).map(fromRest);
      } catch {
        messages = [];
      } finally {
        loading = false;
        scrollToEnd();
      }
    })();

    const off = getWebSocketClient().on('workroom_message', (data: any) => {
      if (data?.channelId !== room.channelId) return;
      const text = data.text ?? '';
      if (!text) return;
      // The owner's own send already rendered optimistically — don't double it.
      if (messages.some((m) => m.mine && m.content === text)) return;
      messages = [
        ...messages,
        {
          id: crypto.randomUUID(),
          from: data.senderName || data.from || '',
          content: text,
          mine: false,
        },
      ];
      scrollToEnd();
    });
    return off;
  });

  async function send() {
    const text = draft.trim();
    if (!text || sending) return;
    sending = true;
    try {
      await sendWorkroomMessage(room.channelId, { text });
      messages = [...messages, { id: crypto.randomUUID(), from: '', content: text, mine: true }];
      draft = '';
      scrollToEnd();
    } catch {
      /* keep the draft; the composer state is the error surface */
    } finally {
      sending = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }
</script>

<div class="flex-1 min-w-0 min-h-0 flex flex-col">
  {#if room.mission}
    <div class="shrink-0 px-5 py-2 border-b border-base-300 text-xs text-base-content/60 truncate">
      {room.mission}
    </div>
  {/if}

  <div bind:this={scroller} class="flex-1 min-h-0 overflow-y-auto px-5 py-4">
    {#if loading}
      <div class="flex justify-center py-16">
        <span class="loading loading-spinner loading-md text-primary"></span>
      </div>
    {:else if messages.length === 0}
      <div class="flex flex-col items-center justify-center py-16 text-center px-6">
        <p class="text-sm font-medium">{$t('workrooms.emptyTitle')}</p>
        <p class="text-xs text-base-content/50 mt-1 max-w-sm">{$t('workrooms.emptyHint')}</p>
      </div>
    {:else}
      <div class="max-w-2xl mx-auto flex flex-col gap-4" data-selectable>
        {#each messages as m (m.id)}
          <div class="flex flex-col {m.mine ? 'items-end' : 'items-start'}">
            <div class="flex items-baseline gap-2 mb-1 {m.mine ? 'flex-row-reverse' : ''}">
              <span class="text-xs font-medium text-base-content/70">
                {m.mine ? $t('workrooms.you') : m.from}
              </span>
            </div>
            <div class="max-w-[85%] rounded-2xl px-4 py-2.5 text-sm leading-relaxed prose prose-sm {m.mine
              ? 'bg-primary/10 rounded-tr-sm'
              : 'bg-base-200 rounded-tl-sm'}">
              {@html parseMarkdown(m.content)}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- The defining affordance: the owner speaks here. -->
  <div class="shrink-0 border-t border-base-300 px-4 py-3">
    <div class="max-w-2xl mx-auto flex items-end gap-2">
      <textarea
        rows="1"
        class="textarea textarea-bordered flex-1 min-h-10 max-h-40 text-sm leading-relaxed resize-none"
        placeholder={$t('workrooms.composerPlaceholder')}
        bind:value={draft}
        {onkeydown}
      ></textarea>
      <button
        type="button"
        class="btn btn-primary btn-sm h-10 rounded-field px-4"
        disabled={sending || !draft.trim()}
        onclick={send}
      >
        {#if sending}<span class="loading loading-spinner loading-xs"></span>{/if}
        {$t('workrooms.send')}
      </button>
    </div>
  </div>
</div>
