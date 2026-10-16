<!--
  TeamView — a team's conversation. A team is a local object: its thread
  lives on this Nebo, every member reads every post, and the owner's
  composer is LIVE here — post the mission, @-name a member to ask them to
  act, redirect mid-flight.

  Data: initial load from GET /teams/{id}/messages, then `team_message`
  WS events (never polling). A post with no mention lets every member
  answer once; a mentioned member is asked to act. A command the owner
  types (/clear, /stop) is run by the server and never posted; its answer
  shows above the composer, and a clear leaves its divider in the thread.

  The working strip above the composer: who is working in this team's
  conversation now (GET /teams/{id}/working, then the work's own events),
  folded to "N working"; a row opens what that one is doing in a sheet.

  With `project` set it is the SAME view bound to one of the team's
  projects: its own thread (GET/POST /projects/{id}/messages,
  `project_message`), its working list and Stop (/projects/{id}/working,
  /stop, `project_activity`). The lead's lines that hand work out are the
  project's cards (`HandoffLine` with `card`). A done project says so in
  its composer: writing there reopens it. The team's own conversation is
  untouched by any of it.

  The rail beside the thread (behind the header toggle on phones) holds
  what doesn't change for the team (its standing rules, edited with the
  team), its projects (going first, done ones folded under "Done · N"),
  "+ New project", and who is on the team.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, tick, untrack } from 'svelte';
  import { slide } from 'svelte/transition';
  import Crown from 'lucide-svelte/icons/crown';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import Pencil from 'lucide-svelte/icons/pencil';
  import NewTeamModal from '$lib/components/teams/NewTeamModal.svelte';
  import NewProjectModal from '$lib/components/teams/NewProjectModal.svelte';
  import HandoffLine from '$lib/components/handoffs/HandoffLine.svelte';
  import Plus from 'lucide-svelte/icons/plus';
  import { goto } from '$lib/nav';
  import {
    getProjectMessages, getTeamMessages, projectWorking, sendProjectMessage, sendTeamMessage,
    stopProjectWork, stopTeamWork, teamWorking,
  } from '$lib/api/nebo';
  import { isStorageFull, storageFullMessage, uploadFiles } from '$lib/api/upload';
  import { stripAttachmentNotes, type UploadedAttachment } from '$lib/types/attachment';
  import type { Project, Team, TeamCard, TeamMessage } from '$lib/api/neboComponents';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { parseMarkdown } from '$lib/markdown';
  import { EVERYONE, renderMentionChips } from '$lib/mentions';
  import TranscriptMessage from '$lib/components/chat/TranscriptMessage.svelte';
  import ChatComposer from '$lib/components/chat/ChatComposer.svelte';
  import TeamWorkSheet from '$lib/components/teams/TeamWorkSheet.svelte';
  import { applyWorkEvent, stopBody, workKey, WORK_EVENTS, type WorkEntry } from '$lib/teams/teamWork';
  import { splitProjects } from '$lib/teams/projects';
  import { projectKey, teamKey } from '$lib/chat/sessionKey';
  import { callWords } from '$lib/chat/callLabel';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';

  let {
    team,
    roster = [],
    projects = [],
    project = null,
    onprojectcreated,
  }: {
    team: Team;
    /** The employee roster, for member chips and sender-name resolution. */
    roster?: { id: string; name: string; initial: string; color?: string; shape?: string; loopAgentId?: string; isApp?: boolean }[];
    /** The team's projects, in the server's order (the page keeps them live). */
    projects?: Project[];
    /** The project whose thread this is; null for the team's own. The view
     *  is keyed per thread, so which thread never changes under it. */
    project?: Project | null;
    /** A project the owner just started here. */
    onprojectcreated?: (project: Project) => void;
  } = $props();

  // Which thread this view is: the team's own, or one of its projects'.
  const projectId = untrack(() => project?.id ?? '');
  const thread = untrack(() => (projectId ? projectKey(team.id, projectId) : teamKey(team.id)));

  // The member picker, in edit mode. The server broadcasts team_updated on
  // save; the shell patches its list and this view re-renders from `team`.
  let editOpen = $state(false);
  let newProjectOpen = $state(false);
  let doneOpen = $state(false);
  const projectLists = $derived(splitProjects(projects));
  const openProject = (id: string) => goto(`/teams/${team.id}?project=${id}`);

  type TeamMsg = {
    id: string;
    from: string;
    content: string;
    mine: boolean;
    attachments: UploadedAttachment[];
    /** The divider where the owner cleared the team's conversation. */
    cleared?: boolean;
    /** The card this post hands out (a project's lead's line). */
    card?: TeamCard;
  };
  const asAttachments = (v: unknown): UploadedAttachment[] =>
    Array.isArray(v) ? (v as UploadedAttachment[]) : [];

  // Each employee wears its roster character in the team, so the owner can
  // tell who's who at a glance — the same avatar as the sidebar.
  const rosterFor = (label: string) =>
    roster.find((a) => a.id === label || a.name === label);

  let messages: TeamMsg[] = $state([]);
  // Who is working in the team's conversation now: members and the helpers
  // they started there (`teamWork`). Folded to one line until opened.
  let work: WorkEntry[] = $state([]);
  let workOpen = $state(false);
  let sheetEntry: WorkEntry | null = $state(null);
  // The answer to a command or a stop, shown above the composer until the
  // next send.
  let notice = $state('');
  // Redirect: the composer opens with the member named.
  let prefill = $state('');
  let loading = $state(true);
  let sending = $state(false);
  let scroller = $state<HTMLDivElement | null>(null);
  // Phones tuck the member list behind the header toggle; desktop shows the
  // rail permanently.
  let membersOpen = $state(false);

  // The standard composer's mention autocomplete wants the roster in its
  // AgentInfo shape: @everyone first, always, then the members A to Z, since
  // only members can be asked to act.
  const composerAgents = $derived([
    { id: EVERYONE, name: 'everyone', role: $t('teams.everyoneHint'), initial: '@', status: 'online', color: 'teal' },
    ...roster
      .filter((a) => team.members.some((m) => m.agentId === a.id))
      .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }))
      .map((a) => ({
        id: a.id,
        name: a.name,
        role: '',
        initial: a.initial,
        status: 'online',
        color: a.color ?? 'teal',
      })),
  ]);

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
    team.members
      .map(
        (m) =>
          roster.find((a) => a.id === m.agentId) ?? {
            // A member on another computer has no local roster row; the name
            // recorded when they joined is the only label there is.
            id: m.agentId,
            name: m.name || m.agentId,
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
    ...(m.cleared ? { cleared: true } : {}),
    ...(m.card ? { card: m.card } : {}),
  });

  // The thread's own doors: the team's, or its project's.
  const readMessages = async () => (projectId ? await getProjectMessages(projectId) : await getTeamMessages(team.id))?.messages ?? [];
  const readWorking = async () => (projectId ? await projectWorking(projectId) : await teamWorking(team.id))?.working ?? [];
  const stopIn = (body: { agentId?: string; taskId?: string }) =>
    projectId ? stopProjectWork(projectId, body) : stopTeamWork(team.id, body);

  async function stopWork(entry?: WorkEntry) {
    sheetEntry = null;
    try {
      const resp = await stopIn(entry ? stopBody(entry) : {});
      notice = resp?.message ?? '';
    } catch {
      notice = '';
    }
  }

  function redirect(entry: WorkEntry) {
    sheetEntry = null;
    prefill = `@${entry.member} `;
  }

  async function scrollToEnd() {
    await tick();
    scroller?.scrollTo({ top: scroller.scrollHeight });
  }

  onMount(() => {
    (async () => {
      try {
        messages = (await readMessages()).map(fromRest);
      } catch {
        messages = [];
      } finally {
        loading = false;
        scrollToEnd();
      }
    })();

    (async () => {
      const working = await readWorking().catch(() => null);
      if (working) work = working;
    })();
    const ws = getWebSocketClient();
    const offWork = WORK_EVENTS.map((type) =>
      ws.on(type, (data: any) => {
        work = applyWorkEvent(work, type, data ?? {}, thread, nameFor);
      })
    );

    // The team's own posts come as `team_message`, a project's as
    // `project_message`: each view hears its own thread only.
    const off = ws.on(projectId ? 'project_message' : 'team_message', (data: any) => {
      if (data?.teamId !== team.id || (projectId && data?.projectId !== projectId)) return;
      const text = data.text ?? '';
      if (!text) return;
      // Already here: the same row can arrive twice (the owner's own echo
      // after its optimistic render, a reconnect replay).
      if (data.messageId && messages.some((m) => m.id === data.messageId)) return;
      if (data.cleared) {
        messages = [...messages, { id: data.messageId || crypto.randomUUID(), from: '', content: text, mine: false, attachments: [], cleared: true }];
        scrollToEnd();
        return;
      }
      const senderName = data.senderName || nameFor(data.fromAgentId || data.from || '');
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
          ...(data.card ? { card: data.card as TeamCard } : {}),
        },
      ];
      scrollToEnd();
    });

    return () => {
      off();
      offWork.forEach((fn) => fn());
    };
  });

  async function send(raw: string, files: { file: File }[] = []) {
    const text = raw.trim();
    if ((!text && files.length === 0) || sending) return;
    sending = true;
    notice = '';
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
      const resp = projectId
        ? await sendProjectMessage(projectId, { text, attachments })
        : await sendTeamMessage(team.id, { text, attachments });
      if (resp && 'command' in resp && resp.command) {
        // A command, run by the server and never posted: its answer shows
        // above the composer; a clear's divider arrives as its own row.
        messages = messages.filter((m) => m.id !== tempId);
        notice = resp.message;
      } else if (resp?.messageId) {
        // The echo may have landed as its own row while this request was in
        // flight: keep one row under the server's id, never two.
        messages = messages.some((m) => m.id === resp.messageId)
          ? messages.filter((m) => m.id !== tempId)
          : messages.map((m) => (m.id === tempId ? { ...m, id: resp.messageId } : m));
      }
    } catch (e) {
      // The post did not land: take the row back so the thread stays honest.
      messages = messages.filter((m) => m.id !== tempId);
      // A full account is said plainly where the post was made.
      if (isStorageFull(e)) notice = storageFullMessage(e.message);
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

{#snippet standingRows()}
  <!-- What doesn't change for the team: every project starts with it.
       Edited with the rest of the team. -->
  <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45 px-3 mb-1">{$t('teams.standing')}</span>
  {#if team.standing}
    <button
      type="button"
      class="mx-1.5 px-1.5 py-1 rounded-md bg-transparent border-none cursor-pointer text-start text-xs text-base-content/70 leading-relaxed whitespace-pre-line hover:bg-base-200"
      onclick={() => (editOpen = true)}
      title={$t('teams.editMembers')}
    >{team.standing}</button>
  {:else}
    <button
      type="button"
      class="mx-1.5 px-1.5 py-1 rounded-md bg-transparent border-none cursor-pointer text-start text-xs text-base-content/45 leading-relaxed hover:bg-base-200 hover:text-base-content/70"
      onclick={() => (editOpen = true)}
    >{$t('teams.standingEmpty')}</button>
  {/if}
{/snippet}

{#snippet projectRow(p: Project)}
  <button
    type="button"
    class="w-full flex items-center gap-2 px-3 py-1.5 bg-transparent border-none cursor-pointer text-start min-w-0 {p.id === projectId ? 'bg-primary/10' : 'hover:bg-base-200'}"
    onclick={() => openProject(p.id)}
    aria-current={p.id === projectId ? 'page' : undefined}
  >
    <span class="text-sm truncate min-w-0 flex-1">{p.title}</span>
    <span class="project-chip project-chip-{p.state}">{$t(`teams.state.${p.state}`)}</span>
  </button>
{/snippet}

{#snippet projectRows()}
  <!-- The team's projects: the ones going first; done ones folded. -->
  <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45 px-3 mb-1">{$t('teams.projects')}</span>
  {#each projectLists.going as p (p.id)}
    {@render projectRow(p)}
  {/each}
  {#if projectLists.done.length > 0}
    <button
      type="button"
      class="w-full flex items-center gap-1.5 px-3 py-1.5 bg-transparent border-none cursor-pointer text-start text-xs text-base-content/55 hover:text-base-content"
      aria-expanded={doneOpen}
      onclick={() => (doneOpen = !doneOpen)}
    >
      <span>{$t('teams.doneCount', { values: { count: projectLists.done.length } })}</span>
      <ChevronDown class="w-3 h-3 shrink-0 transition-transform {doneOpen ? 'rotate-180' : ''}" />
    </button>
    {#if doneOpen || projectLists.done.some((p) => p.id === projectId)}
      <div transition:slide={{ duration: 140 }}>
        {#each projectLists.done as p (p.id)}
          {@render projectRow(p)}
        {/each}
      </div>
    {/if}
  {/if}
  <div class="px-3 pt-1">
    <button type="button" class="btn btn-ghost btn-xs gap-1 text-base-content/70" onclick={() => (newProjectOpen = true)}>
      <Plus class="w-3.5 h-3.5" />
      {$t('teams.newProject')}
    </button>
  </div>
{/snippet}

{#snippet memberRows()}
  <!-- One vertical row per member; the organizer (the employee that created
       the team) wears the crown. -->
  {#each members as m (m.id)}
    <div class="flex items-center gap-2.5 px-3 py-1.5 min-w-0">
      <AgentAvatar name={m.name} id={m.id} color={rosterFor(m.id)?.color} shape={rosterFor(m.id)?.shape} size="xs" />
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
      class="w-full flex items-center gap-2.5 px-4 py-2 bg-transparent border-none cursor-pointer text-start min-w-0"
      onclick={() => (membersOpen = !membersOpen)}
    >
      <span class="flex -space-x-1.5 shrink-0">
        {#each members.slice(0, 3) as m (m.id)}
          <AgentAvatar name={m.name} id={m.id} color={rosterFor(m.id)?.color} shape={rosterFor(m.id)?.shape} size="xs" />
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
      <div transition:slide={{ duration: 160 }} class="pb-2 flex flex-col max-h-[60vh] overflow-y-auto">
        {@render standingRows()}
        <div class="h-2 shrink-0"></div>
        {@render projectRows()}
        <div class="h-2 shrink-0"></div>
        <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45 px-3 mb-1">{$t('teams.inTeam')}</span>
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
          {#if m.cleared}
            <div class="divider my-2 text-xs text-base-content/50">{$t('chat.clearedBoundary')}</div>
          {:else if m.card}
            <!-- The lead hands work out: the line is the card, its state
                 read from the hand-off it carries. -->
            <div class="flex flex-col items-start">
              <HandoffLine
                card
                to={nameFor(m.card.agentId)}
                ask={m.card.task}
                handoffId={m.card.handoffId}
                doneWhen={m.card.doneWhen}
              />
            </div>
          {:else}
          <TranscriptMessage
            name={m.mine ? $t('teams.you') : m.from}
            mine={m.mine}
            avatar={sender ?? null}
            html={renderMentionChips(parseMarkdown(stripAttachmentNotes(m.content)), roster)}
            attachments={m.attachments}
          />
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <!-- The defining affordance: the owner speaks here — through the ONE
       standard chat input, so @-mentions autocomplete against the members
       and the team feels like every other conversation in the app. -->
  <div class="shrink-0 px-4 pb-3 pt-1">
    <div class="max-w-2xl mx-auto">
      {#if work.length > 0}
        <!-- Who is working in this conversation: one quiet line, opened to
             one line per member or helper; a line opens its sheet. -->
        <div class="mb-1.5">
          <button
            type="button"
            class="w-full flex items-center gap-2 px-1 py-1 bg-transparent border-none cursor-pointer text-start text-xs text-base-content/60 hover:text-base-content"
            aria-expanded={workOpen}
            onclick={() => (workOpen = !workOpen)}
          >
            <span class="w-2 h-2 rounded-full bg-success agent-working-dot shrink-0"></span>
            <span>{$t('teams.workingCount', { values: { count: work.length } })}</span>
            <ChevronDown class="w-3 h-3 shrink-0 transition-transform {workOpen ? 'rotate-180' : ''}" />
          </button>
          {#if workOpen}
            <div transition:slide={{ duration: 140 }} class="flex flex-col">
              {#each work as w (workKey(w))}
                <button
                  type="button"
                  class="w-full flex items-center gap-2 px-1 py-1 rounded-md bg-transparent border-none cursor-pointer text-start min-w-0 hover:bg-base-200"
                  onclick={() => (sheetEntry = w)}
                >
                  <span class="w-1.5 h-1.5 rounded-full bg-success agent-working-dot shrink-0"></span>
                  <span class="text-xs font-medium shrink-0 max-w-[40%] truncate">{w.title}</span>
                  <span class="text-xs text-base-content/55 truncate min-w-0">{(w.call ? callWords(w.call, $t) : w.activity) || $t('sidebar.working')}</span>
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
      {#if notice}
        <p class="px-1 pb-1.5 text-xs text-base-content/60">{notice}</p>
      {/if}
      <ChatComposer
        agentId="team"
        threadId={projectId || team.id}
        teamId={team.id}
        placeholder={project
          ? project.state === 'done' ? $t('teams.reopenPlaceholder') : $t('teams.projectComposerPlaceholder')
          : $t('teams.composerPlaceholder')}
        allAgents={composerAgents}
        isLoading={sending || work.length > 0}
        onsend={(text, files) => send(text, files)}
        onstop={() => stopWork()}
        {prefill}
        onprefilled={() => (prefill = '')}
      />
    </div>
  </div>
</div>

<!-- Desktop: who's on the team, vertically — a door list, not a banner. -->
<aside class="hidden md:flex w-56 shrink-0 border-s border-base-300 min-h-0 flex-col overflow-y-auto py-3">
  <!-- Fixed three-line well: a long mission scrolls inside it instead of
       shoving the member list down; a short one keeps the same height so the
       rail never jumps between teams. -->
  <div class="shrink-0 h-[4.5rem] overflow-y-auto px-3 border-b border-base-300 mb-2">
    <p class="text-xs text-base-content/60 leading-relaxed">{team.mission}</p>
  </div>
  {@render standingRows()}
  <div class="h-3 shrink-0"></div>
  {@render projectRows()}
  <div class="h-3 shrink-0"></div>
  <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45 px-3 mb-1">{$t('teams.inTeam')}</span>
  {@render memberRows()}
  <div class="px-3 pt-2">{@render editMembersButton()}</div>
</aside>
</div>

{#if editOpen}
  <NewTeamModal {roster} {team} onclose={() => (editOpen = false)} oncreated={() => (editOpen = false)} />
{/if}

{#if newProjectOpen}
  <NewProjectModal
    {team}
    onclose={() => (newProjectOpen = false)}
    oncreated={(p) => {
      newProjectOpen = false;
      onprojectcreated?.(p);
      openProject(p.id);
    }}
    onpicklead={() => {
      newProjectOpen = false;
      editOpen = true;
    }}
  />
{/if}

{#if sheetEntry}
  <!-- The row as it is now: its activity line keeps moving under the sheet. -->
  {@const current = work.find((w) => sheetEntry && workKey(w) === workKey(sheetEntry))}
  {#key workKey(sheetEntry)}
    <TeamWorkSheet
      {readWorking}
      entry={current ?? sheetEntry}
      live={!!current}
      onclose={() => (sheetEntry = null)}
      onstop={(entry) => stopWork(entry)}
      onredirect={redirect}
    />
  {/key}
{/if}
