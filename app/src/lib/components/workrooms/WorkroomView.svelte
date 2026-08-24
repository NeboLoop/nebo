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
  import { slide } from 'svelte/transition';
  import Crown from 'lucide-svelte/icons/crown';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import { getWorkroomMessages, sendWorkroomMessage } from '$lib/api/nebo';
  import type { Workroom, WorkroomMessage } from '$lib/api/neboComponents';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { parseMarkdown } from '$lib/markdown';
  import { renderMentionChips } from '$lib/mentions';
  import TranscriptMessage from '$lib/components/chat/TranscriptMessage.svelte';
  import ChatComposer from '$lib/components/chat/ChatComposer.svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';

  let {
    room,
    roster = [],
  }: {
    room: Workroom;
    /** The employee roster, for member chips and sender-name resolution. */
    roster?: { id: string; name: string; initial: string; color?: string; loopAgentId?: string }[];
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
  // Members currently running because of a room dispatch — the owner's proof
  // a message was picked up. Set by workroom_activity, cleared by the reply.
  let working: { id: string; name: string }[] = $state([]);
  let loading = $state(true);
  let sending = $state(false);
  let scroller = $state<HTMLDivElement | null>(null);
  // Phones tuck the member list behind the header toggle; desktop shows the
  // rail permanently.
  let membersOpen = $state(false);

  // The standard composer's mention autocomplete wants the roster in its
  // AgentInfo shape.
  const composerAgents = $derived(
    roster.map((a) => ({
      id: a.id,
      name: a.name,
      role: '',
      initial: a.initial,
      status: 'online',
      color: a.color ?? 'teal',
    }))
  );

  // Dedupe key for the owner's optimistic send vs its wire echo: the wire copy
  // may carry rewritten hub-grammar tokens AND a server-prepended organizer
  // mention (unaddressed sends route to the organizer), so mention tokens are
  // stripped entirely — only the human-typed text has to match.
  const strippedText = (text: string) =>
    text.replace(/<@[A-Za-z0-9._-]+>/g, '').replace(/\s+/g, ' ').trim();

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
    roster.find((a) => a.id === from || a.name === from || a.loopAgentId === from)?.name ?? from;

  // The hub's history rows carry `role` for human-injected legs; live WS
  // events carry senderName. Both normalize to the same shape. Content stays
  // RAW — mention tokens render as chips at display time.
  const fromRest = (m: WorkroomMessage): RoomMsg => ({
    id: m.id,
    from: nameFor(m.from),
    content: m.content,
    mine: m.role === 'user',
  });

  function clearWorking(senderName: string, fromAgentId?: string) {
    working = working.filter((w) => w.name !== senderName && w.id !== (fromAgentId || ''));
  }

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
      const senderName = data.senderName || nameFor(data.fromAgentId || data.from || '');
      clearWorking(senderName, data.fromAgentId);
      // The owner's wire echo comes back senderName "Owner" — it is MINE, and
      // it already rendered optimistically. Drop the duplicate; keep it (as a
      // mine bubble) only if another device sent it.
      const isOwner = senderName === 'Owner';
      if (isOwner && messages.some((m) => m.mine && strippedText(m.content) === strippedText(text)))
        return;
      messages = [
        ...messages,
        {
          id: crypto.randomUUID(),
          from: isOwner ? '' : senderName,
          content: text,
          mine: isOwner,
        },
      ];
      scrollToEnd();
    });

    // "Somebody picked your message up": the server broadcasts when a room
    // dispatch starts an agent run; the reply's workroom_message clears it.
    const offActivity = getWebSocketClient().on('workroom_activity', (data: any) => {
      if (data?.channelId !== room.channelId || data?.state !== 'started') return;
      const id = data.agentId || data.agentName;
      if (!id || working.some((w) => w.id === id)) return;
      working = [...working, { id, name: data.agentName || nameFor(id) }];
      scrollToEnd();
      // Failsafe: a run that dies without posting must not spin forever.
      setTimeout(() => {
        working = working.filter((w) => w.id !== id);
      }, 300_000);
    });

    return () => {
      off();
      offActivity();
    };
  });

  async function send(raw: string) {
    const text = raw.trim();
    if (!text || sending) return;
    sending = true;
    try {
      await sendWorkroomMessage(room.channelId, { text });
      messages = [
        ...messages,
        { id: crypto.randomUUID(), from: '', content: text, mine: true },
      ];
      scrollToEnd();
    } catch {
      /* the composer keeps focus; a failed send simply doesn't render */
    } finally {
      sending = false;
    }
  }
</script>

{#snippet memberRows()}
  <!-- One vertical row per member; the first is the organizer — the employee
       that opened the room (the creation tool writes the creator first). -->
  {#each members as m, i (m.id)}
    <div class="flex items-center gap-2.5 px-3 py-1.5 min-w-0">
      <span class="w-6 h-6 rounded-full flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {colorClass(rosterFor(m.id)?.color)}">{m.initial}</span>
      <span class="text-sm truncate min-w-0">{m.name}</span>
      {#if i === 0}
        <span class="tooltip tooltip-left shrink-0 text-warning/80" data-tip={$t('workrooms.organizer')}>
          <Crown class="w-3.5 h-3.5" />
        </span>
      {/if}
    </div>
  {/each}
{/snippet}

<div class="flex-1 min-w-0 min-h-0 flex">
<div class="flex-1 min-w-0 min-h-0 flex flex-col">
  <!-- Phone: the member list lives behind the header toggle. -->
  <div class="md:hidden shrink-0 border-b border-base-300">
    <button
      type="button"
      class="w-full flex items-center gap-2.5 px-4 py-2 bg-transparent border-none cursor-pointer text-left min-w-0"
      onclick={() => (membersOpen = !membersOpen)}
    >
      <span class="flex -space-x-1.5 shrink-0">
        {#each members.slice(0, 3) as m (m.id)}
          <span class="w-5 h-5 rounded-full border border-base-100 flex items-center justify-center font-mono text-[9px] font-semibold {colorClass(rosterFor(m.id)?.color)}">{m.initial}</span>
        {/each}
      </span>
      <span class="text-xs text-base-content/60 shrink-0">{$t('workrooms.membersCount', { values: { count: members.length } })}</span>
      {#if room.mission}
        <span class="text-xs text-base-content/50 truncate min-w-0">{room.mission}</span>
      {/if}
      <span class="flex-1"></span>
      <ChevronDown class="w-3.5 h-3.5 shrink-0 text-base-content/50 transition-transform {membersOpen ? 'rotate-180' : ''}" />
    </button>
    {#if membersOpen}
      <div transition:slide={{ duration: 160 }} class="pb-2">
        {@render memberRows()}
      </div>
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
            html={renderMentionChips(parseMarkdown(m.content), roster)}
          />
        {/each}
        <!-- Who is on it right now — the owner's proof a send was picked up. -->
        {#each working as w (w.id)}
          {@const wa = rosterFor(w.id) ?? rosterFor(w.name)}
          <div class="flex items-center gap-2.5">
            <span class="w-6 h-6 rounded-full flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {colorClass(wa?.color)}">{wa?.initial ?? (w.name[0] ?? '?').toUpperCase()}</span>
            <span class="text-xs text-base-content/60">{$t('workrooms.working', { values: { name: w.name } })}</span>
            <span class="loading loading-dots loading-xs text-base-content/50"></span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- The defining affordance: the owner speaks here — through the ONE
       standard chat input, so @-mentions autocomplete against the roster and
       the room feels like every other conversation in the app. -->
  <div class="shrink-0 px-4 pb-3 pt-1">
    <div class="max-w-2xl mx-auto">
      <ChatComposer
        placeholder={$t('workrooms.composerPlaceholder')}
        allAgents={composerAgents}
        allowAttachments={false}
        isLoading={sending}
        onsend={(text) => send(text)}
      />
    </div>
  </div>
</div>

<!-- Desktop: who's in the room, vertically — a door list, not a banner. -->
<aside class="hidden md:flex w-56 shrink-0 border-l border-base-300 min-h-0 flex-col overflow-y-auto py-3">
  <!-- Fixed three-line well: a long mission scrolls inside it instead of
       shoving the member list down; a short one keeps the same height so the
       rail never jumps between rooms. -->
  <div class="shrink-0 h-[4.5rem] overflow-y-auto px-3 border-b border-base-300 mb-2">
    <p class="text-xs text-base-content/60 leading-relaxed">{room.mission}</p>
  </div>
  <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45 px-3 mb-1">{$t('workrooms.inRoom')}</span>
  {@render memberRows()}
</aside>
</div>
