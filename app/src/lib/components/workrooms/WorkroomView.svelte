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
  import TranscriptMessage from '$lib/components/chat/TranscriptMessage.svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';

  let {
    room,
    roster = [],
  }: {
    room: Workroom;
    /** The employee roster, for member chips and sender-name resolution. */
    roster?: { id: string; name: string; initial: string; color?: string }[];
  } = $props();

  type RoomMsg = {
    id: string;
    from: string;
    content: string;
    mine: boolean;
  };

  // Each employee keeps its roster color in the room, so the owner can tell
  // who's who at a glance — same palette as the sidebar avatars.
  const colorClass = (color?: string) => {
    const ac = AGENT_COLORS_MAP[color ?? ''] ?? AGENT_COLORS_MAP['teal'];
    return `${ac.bgClass} ${ac.inkClass}`;
  };
  const rosterFor = (label: string) =>
    roster.find((a) => a.id === label || a.name === label);

  let messages: RoomMsg[] = $state([]);
  let loading = $state(true);
  let draft = $state('');
  let sending = $state(false);
  let scroller = $state<HTMLDivElement | null>(null);

  // Who is in the room — resolved against the roster; unknown ids (a cloud
  // coworker, a departed employee) keep their raw label rather than vanishing.
  const members = $derived(
    room.memberAgentIds.map(
      (id) =>
        roster.find((a) => a.id === id) ?? {
          id,
          name: id,
          initial: (id[0] ?? '?').toUpperCase(),
        }
    )
  );

  // The hub speaks in ids; the owner reads names.
  const nameFor = (from: string) =>
    roster.find((a) => a.id === from || a.name === from)?.name ?? from;

  // The hub's history rows carry `role` for human-injected legs; live WS
  // events carry senderName. Both normalize to the same shape.
  const fromRest = (m: WorkroomMessage): RoomMsg => ({
    id: m.id,
    from: nameFor(m.from),
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
          from: data.senderName || nameFor(data.fromAgentId || data.from || ''),
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
  <!-- Who's in the room + what it's for. -->
  <div class="shrink-0 px-5 py-2.5 border-b border-base-300 flex items-center gap-3 min-w-0">
    <div class="flex items-center gap-1.5 shrink-0">
      {#each members as m (m.id)}
        <span class="inline-flex items-center gap-1.5 pl-1 pr-2.5 py-0.5 rounded-full bg-base-200 text-xs">
          <span class="w-5 h-5 rounded-full flex items-center justify-center font-mono text-[10px] font-semibold {colorClass(rosterFor(m.id)?.color)}">{m.initial}</span>
          {m.name}
        </span>
      {/each}
    </div>
    {#if room.mission}
      <span class="text-xs text-base-content/60 truncate">{room.mission}</span>
    {/if}
  </div>

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
          {@const sender = m.mine ? undefined : rosterFor(m.from)}
          <TranscriptMessage
            name={m.mine ? $t('workrooms.you') : m.from}
            mine={m.mine}
            initial={sender?.initial ?? ''}
            avatarClass={sender ? colorClass(sender.color) : ''}
            html={parseMarkdown(m.content)}
          />
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
