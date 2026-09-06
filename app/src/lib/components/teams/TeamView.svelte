<!--
  TeamView — a team's conversation. A team is a local object: its thread
  lives on this Nebo, every member reads every post, and the owner's
  composer is LIVE here — post the mission, @-name a member to ask them to
  act, redirect mid-flight.

  Data: initial load from GET /teams/{id}/messages, then `team_message`
  WS events (never polling). A post with no mention lets every member
  answer once; a mentioned member is asked to act.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, tick } from 'svelte';
  import { slide } from 'svelte/transition';
  import Crown from 'lucide-svelte/icons/crown';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import Pencil from 'lucide-svelte/icons/pencil';
  import NewTeamModal from '$lib/components/teams/NewTeamModal.svelte';
  import { getTeamMessages, sendTeamMessage } from '$lib/api/nebo';
  import { uploadFiles } from '$lib/api/upload';
  import { stripAttachmentNotes, type UploadedAttachment } from '$lib/types/attachment';
  import type { Team, TeamMessage } from '$lib/api/neboComponents';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { parseMarkdown } from '$lib/markdown';
  import { renderMentionChips } from '$lib/mentions';
  import TranscriptMessage from '$lib/components/chat/TranscriptMessage.svelte';
  import ChatComposer from '$lib/components/chat/ChatComposer.svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';

  let {
    team,
    roster = [],
  }: {
    team: Team;
    /** The employee roster, for member chips and sender-name resolution. */
    roster?: { id: string; name: string; initial: string; color?: string; loopAgentId?: string; isApp?: boolean }[];
  } = $props();

  // The member picker, in edit mode. The server broadcasts team_updated on
  // save; the shell patches its list and this view re-renders from `team`.
  let editOpen = $state(false);

  type TeamMsg = {
    id: string;
    from: string;
    content: string;
    mine: boolean;
    attachments: UploadedAttachment[];
  };
  const asAttachments = (v: unknown): UploadedAttachment[] =>
    Array.isArray(v) ? (v as UploadedAttachment[]) : [];

  // Each employee keeps its roster color in the team, so the owner can tell
  // who's who at a glance — same palette as the sidebar avatars.
  const colorClass = (color?: string) => {
    const ac = AGENT_COLORS_MAP[color ?? ''] ?? AGENT_COLORS_MAP['teal'];
    return `${ac.bgClass} ${ac.inkClass}`;
  };
  const rosterFor = (label: string) =>
    roster.find((a) => a.id === label || a.name === label);

  let messages: TeamMsg[] = $state([]);
  // Members currently running because of a post — the owner's proof a post
  // was picked up. Set by team_activity, cleared by the reply.
  let working: { id: string; name: string }[] = $state([]);
  let loading = $state(true);
  let sending = $state(false);
  let scroller = $state<HTMLDivElement | null>(null);
  // Phones tuck the member list behind the header toggle; desktop shows the
  // rail permanently.
  let membersOpen = $state(false);

  // The standard composer's mention autocomplete wants the roster in its
  // AgentInfo shape — members only, since only members can be asked to act.
  const composerAgents = $derived(
    roster
      .filter((a) => team.memberAgentIds.includes(a.id))
      .map((a) => ({
        id: a.id,
        name: a.name,
        role: '',
        initial: a.initial,
        status: 'online',
        color: a.color ?? 'teal',
      }))
  );

  // Dedupe key for the owner's optimistic send vs its server echo: the
  // server copy may carry normalized mention tokens, so tokens are stripped
  // — only the human-typed text has to match.
  // Attachment notes are stripped too: the server appends them to the
  // echoed text, the optimistic row never had them.
  const strippedText = (text: string) =>
    stripAttachmentNotes(text).replace(/<@[A-Za-z0-9._-]+>/g, '').replace(/\s+/g, ' ').trim();

  // Who is on the team — resolved against the roster; unknown ids (a departed
  // employee) keep their raw label rather than vanishing.
  const members = $derived(
    team.memberAgentIds
      .map(
        (id) =>
          roster.find((a) => a.id === id) ?? {
            id,
            name: id,
            initial: (id[0] ?? '?').toUpperCase(),
          }
      )
      .sort((a, b) => a.name.localeCompare(b.name))
  );

  const nameFor = (from: string) =>
    roster.find((a) => a.id === from || a.name === from || a.loopAgentId === from)?.name ?? from;

  // Thread rows carry the sender in `from` (already a name) and `role`;
  // live events carry senderName. Both normalize to the same shape. Content
  // stays RAW — mention tokens render as chips at display time.
  const fromRest = (m: TeamMessage): TeamMsg => ({
    id: m.id,
    from: m.role === 'user' ? '' : nameFor(m.from || m.fromAgentId),
    content: m.content,
    mine: m.role === 'user',
    attachments: asAttachments(m.attachments),
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
        const resp = await getTeamMessages(team.id);
        messages = (resp?.messages ?? []).map(fromRest);
      } catch {
        messages = [];
      } finally {
        loading = false;
        scrollToEnd();
      }
    })();

    const off = getWebSocketClient().on('team_message', (data: any) => {
      if (data?.teamId !== team.id) return;
      const text = data.text ?? '';
      if (!text) return;
      // Already here: the same row can arrive twice (the owner's own echo
      // after its optimistic render, a reconnect replay).
      if (data.messageId && messages.some((m) => m.id === data.messageId)) return;
      const senderName = data.senderName || nameFor(data.fromAgentId || data.from || '');
      clearWorking(senderName, data.fromAgentId);
      // The owner's own post echoes back as role "user" — it is MINE, and
      // it already rendered optimistically. Drop the duplicate; keep it (as
      // a mine bubble) only if another device sent it.
      const isOwner = data.role === 'user' || senderName === 'Owner';
      if (isOwner && messages.some((m) => m.mine && strippedText(m.content) === strippedText(text)))
        return;
      messages = [
        ...messages,
        {
          id: data.messageId || crypto.randomUUID(),
          from: isOwner ? '' : senderName,
          content: text,
          mine: isOwner,
          attachments: asAttachments(data.attachments),
        },
      ];
      scrollToEnd();
    });

    // "Somebody picked your post up": the server broadcasts when a member's
    // run starts; the reply's team_message clears it.
    const offActivity = getWebSocketClient().on('team_activity', (data: any) => {
      if (data?.teamId !== team.id || data?.state !== 'started') return;
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

  async function send(raw: string, files: { file: File }[] = []) {
    const text = raw.trim();
    if ((!text && files.length === 0) || sending) return;
    sending = true;
    // Render first, then send: the server echoes the post over the socket
    // BEFORE the request returns, so the row must already exist for the echo
    // to dedupe against. The temp id is swapped for the server's on return.
    const tempId = crypto.randomUUID();
    messages = [...messages, { id: tempId, from: '', content: text, mine: true, attachments: [] }];
    scrollToEnd();
    try {
      // Same upload step as a direct chat; the server saves and notes them.
      const attachments = files.length ? await uploadFiles(files.map((f) => f.file)) : [];
      if (attachments.length) {
        messages = messages.map((m) => (m.id === tempId ? { ...m, attachments } : m));
      }
      const resp = await sendTeamMessage(team.id, { text, attachments });
      if (resp?.messageId) {
        // The echo may have landed as its own row while this request was in
        // flight: keep one row under the server's id, never two.
        messages = messages.some((m) => m.id === resp.messageId)
          ? messages.filter((m) => m.id !== tempId)
          : messages.map((m) => (m.id === tempId ? { ...m, id: resp.messageId } : m));
      }
    } catch {
      // The post did not land: take the row back so the thread stays honest.
      messages = messages.filter((m) => m.id !== tempId);
    } finally {
      sending = false;
    }
  }
</script>

{#snippet editMembersButton()}
  <button
    type="button"
    class="btn btn-ghost btn-xs gap-1 text-base-content/70"
    onclick={() => (editOpen = true)}
    title={$t('teams.editMembers')}
  >
    <Pencil class="w-3.5 h-3.5" />
    {$t('teams.editMembers')}
  </button>
{/snippet}

{#snippet memberRows()}
  <!-- One vertical row per member; the organizer (the employee that created
       the team) wears the crown. -->
  {#each members as m (m.id)}
    <div class="flex items-center gap-2.5 px-3 py-1.5 min-w-0">
      <span class="w-6 h-6 rounded-full flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {colorClass(rosterFor(m.id)?.color)}">{m.initial}</span>
      <span class="text-sm truncate min-w-0">{m.name}</span>
      {#if m.id === team.organizerAgentId}
        <span class="tooltip tooltip-left shrink-0 text-warning/80" data-tip={$t('teams.organizer')}>
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
      <span class="text-xs text-base-content/60 shrink-0">{$t('teams.membersCount', { values: { count: members.length } })}</span>
      {#if team.mission}
        <span class="text-xs text-base-content/50 truncate min-w-0">{team.mission}</span>
      {/if}
      <span class="flex-1"></span>
      <ChevronDown class="w-3.5 h-3.5 shrink-0 text-base-content/50 transition-transform {membersOpen ? 'rotate-180' : ''}" />
    </button>
    {#if membersOpen}
      <div transition:slide={{ duration: 160 }} class="pb-2">
        {@render memberRows()}
        <div class="px-3 pt-1">{@render editMembersButton()}</div>
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
        <p class="text-sm font-medium">{$t('teams.emptyTitle')}</p>
        <p class="text-xs text-base-content/50 mt-1 max-w-sm">{$t('teams.emptyHint')}</p>
      </div>
    {:else}
      <div class="max-w-2xl mx-auto flex flex-col gap-4" data-selectable>
        {#each messages as m (m.id)}
          {@const sender = m.mine ? undefined : rosterFor(m.from)}
          <TranscriptMessage
            name={m.mine ? $t('teams.you') : m.from}
            mine={m.mine}
            initial={sender?.initial ?? ''}
            avatarClass={sender ? colorClass(sender.color) : ''}
            html={renderMentionChips(parseMarkdown(stripAttachmentNotes(m.content)), roster)}
            attachments={m.attachments}
          />
        {/each}
        <!-- Who is on it right now — the owner's proof a post was picked up. -->
        {#each working as w (w.id)}
          {@const wa = rosterFor(w.id) ?? rosterFor(w.name)}
          <div class="flex items-center gap-2.5">
            <span class="w-6 h-6 rounded-full flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {colorClass(wa?.color)}">{wa?.initial ?? (w.name[0] ?? '?').toUpperCase()}</span>
            <span class="text-xs text-base-content/60">{$t('teams.working', { values: { name: w.name } })}</span>
            <span class="loading loading-dots loading-xs text-base-content/50"></span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- The defining affordance: the owner speaks here — through the ONE
       standard chat input, so @-mentions autocomplete against the members
       and the team feels like every other conversation in the app. -->
  <div class="shrink-0 px-4 pb-3 pt-1">
    <div class="max-w-2xl mx-auto">
      <ChatComposer
        agentId="team"
        threadId={team.id}
        placeholder={$t('teams.composerPlaceholder')}
        allAgents={composerAgents}
        isLoading={sending}
        onsend={(text, files) => send(text, files)}
      />
    </div>
  </div>
</div>

<!-- Desktop: who's on the team, vertically — a door list, not a banner. -->
<aside class="hidden md:flex w-56 shrink-0 border-l border-base-300 min-h-0 flex-col overflow-y-auto py-3">
  <!-- Fixed three-line well: a long mission scrolls inside it instead of
       shoving the member list down; a short one keeps the same height so the
       rail never jumps between teams. -->
  <div class="shrink-0 h-[4.5rem] overflow-y-auto px-3 border-b border-base-300 mb-2">
    <p class="text-xs text-base-content/60 leading-relaxed">{team.mission}</p>
  </div>
  <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45 px-3 mb-1">{$t('teams.inTeam')}</span>
  {@render memberRows()}
  <div class="px-3 pt-2">{@render editMembersButton()}</div>
</aside>
</div>

{#if editOpen}
  <NewTeamModal {roster} {team} onclose={() => (editOpen = false)} oncreated={() => (editOpen = false)} />
{/if}
