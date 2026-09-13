<!--
  The Dashboard: a workforce at work, on one page. Who is working, who needs
  you, how much work happened, how much of it happened without you. Everything
  is read from GET /api/v1/dashboard, which the server composes from what it
  already tracks (the run registry, pending approvals, workflow runs, chat
  history, schedules). Chat stays where it is; each card links back into it.
-->
<script lang="ts">
  import { onMount, onDestroy, getContext } from 'svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import { t } from 'svelte-i18n';
  import Users from 'lucide-svelte/icons/users';
  import Clock from 'lucide-svelte/icons/clock';
  import TrendingUp from 'lucide-svelte/icons/trending-up';
  import BadgeCheck from 'lucide-svelte/icons/badge-check';
  import Bell from 'lucide-svelte/icons/bell';
  import LayoutGrid from 'lucide-svelte/icons/layout-grid';
  import List from 'lucide-svelte/icons/list';
  import Network from 'lucide-svelte/icons/network';
  import Check from 'lucide-svelte/icons/check';
  import { goto } from '$lib/nav';
  import * as api from '$lib/api/nebo';
  import type * as components from '$lib/api/neboComponents';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { assignAgentColors } from '$lib/tokens';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import { formatTime, formatRelative } from '$lib/time';
  import { storage } from '$lib/storage';

  // The phone has no sidebar on screen: the header's back chevron opens the
  // employee list the way every thread page does. The bell opens the Inbox.
  const shell = getContext<AgentPageContext>('agentPage');

  /** How often the page re-reads when no event has arrived. */
  const REFRESH_MS = 15_000;
  const RELOAD_GAP_MS = 1_000;
  /** Height of the activity chart's plot area. */
  const BAR_PX = 120;
  /** Recent runs shown before "Show all". */
  const RECENT_SHOWN = 5;

  type Status = 'all' | 'working' | 'waiting' | 'idle' | 'paused';

  let data = $state<components.DashboardResponse | null>(null);
  // Where Nebo opens: a user preference, so the phone and the desktop agree.
  let startHere = $state(false);
  let error = $state('');
  // Grid or list is a per-device habit, so it lives in base-scoped storage
  // rather than the account's preferences.
  const VIEW_KEY = 'dashboard:view';
  type View = 'grid' | 'list' | 'org';
  const savedView = storage.get(VIEW_KEY);
  let view = $state<View>(savedView === 'list' || savedView === 'org' ? savedView : 'grid');
  $effect(() => storage.set(VIEW_KEY, view));
  let statusFilter = $state<Status>('all');
  let showAllRuns = $state(false);
  let timer: ReturnType<typeof setInterval> | null = null;
  let unsubs: Array<() => void> = [];
  let refreshing = false;

  async function load() {
    if (refreshing) return;
    refreshing = true;
    try {
      data = await api.dashboard();
      error = '';
    } catch (e) {
      error = String(e);
    } finally {
      refreshing = false;
    }
  }

  // One reload per burst of events: the first fires now, the rest of the
  // second folds into a single trailing reload.
  let reloadAt = 0;
  let liveRunIds = '';
  let trailing: ReturnType<typeof setTimeout> | null = null;
  function scheduleLoad() {
    const now = Date.now();
    if (now - reloadAt >= RELOAD_GAP_MS) {
      reloadAt = now;
      load();
      return;
    }
    if (trailing) return;
    trailing = setTimeout(() => {
      trailing = null;
      reloadAt = Date.now();
      load();
    }, RELOAD_GAP_MS - (now - reloadAt));
  }

  async function loadStartPage() {
    try {
      const resp = (await api.userGetPreferences()) as { preferences?: { startPage?: string } | null };
      startHere = resp.preferences?.startPage === 'dashboard';
    } catch {
      startHere = false;
    }
  }
  async function setStartPage(checked: boolean) {
    startHere = checked;
    try {
      await api.userUpdatePreferences({ startPage: checked ? 'dashboard' : 'chat' });
    } catch (e) {
      error = String(e);
      startHere = !checked;
    }
  }

  onMount(() => {
    load();
    loadStartPage();
    timer = setInterval(load, REFRESH_MS);
    const ws = getWebSocketClient();
    // Lifecycle events only: a run starting or ending, an approval asked, an
    // employee paused or resumed. Never the per-token stream or the per-tool
    // event: while a chat runs those arrive every second, and each reload is
    // dozens of queries the server does not need while it is running the chat.
    for (const ev of ['chat_created', 'subagent_start', 'subagent_complete', 'chat_complete', 'chat_error', 'chat_cancelled', 'ask_request', 'approval_request', 'agent_activated', 'agent_deactivated', 'workflow_run_started', 'workflow_activity_update', 'workflow_run_completed', 'workflow_run_failed', 'workflow_run_exited']) {
      unsubs.push(ws.on(ev, scheduleLoad));
    }
    // The five-second progress snapshot lists every live run. It only earns a
    // reload when that set changes (a start or end this page missed); the live
    // line's elapsed time catches up on the regular poll.
    unsubs.push(ws.on('agent_progress', (msg: { runs?: Array<{ runId: string }> }) => {
      const ids = (msg?.runs ?? []).map((r) => r.runId).sort().join(',');
      if (ids === liveRunIds) return;
      liveRunIds = ids;
      scheduleLoad();
    }));
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
    if (trailing) clearTimeout(trailing);
    for (const off of unsubs) off();
  });

  // The same assignment the sidebar makes (user-set colour first, then a
  // stable fallback per employee), so a card matches its row.
  const colors = $derived(assignAgentColors(data?.employees ?? []));
  function outcomeClass(outcome: string) {
    return outcome === 'done' ? 'text-success' : outcome === 'stopped' ? 'text-error' : outcome === 'waiting' ? 'text-warning' : outcome === 'skipped' ? 'text-base-content/60' : 'text-success';
  }
  function openChat(e: components.DashboardEmployee) {
    // An isolated employee keeps one thread per matter: open its list, the
    // same screen the sidebar's chevron opens.
    if (e.isolated) return goto(`/${e.id}/threads?list=${e.id}`);
    goto(e.chatId ? `/${e.id}/threads/${e.chatId}` : `/${e.id}/threads`);
  }
  function openRuns(agentId: string) {
    // The runs sheet opens over the dashboard; the shell reads ?agent for whose.
    goto(`/dashboard?runs=1&agent=${agentId}`);
  }
  /** A working employee's primary action: the run it is on, else its chat. */
  function openWork(e: components.DashboardEmployee) {
    if (e.runId) return goto(`/dashboard?runs=1&agent=${e.id}&run=${e.runId}`);
    openChat(e);
  }
  async function answer(a: components.DashboardApproval, approved: boolean) {
    if (a.kind === 'tool') {
      getWebSocketClient().send('approval_response', { request_id: a.id, approved, always: false });
    } else {
      try {
        await api.resolveWorkflowApproval(a.id, { approved });
      } catch (e) {
        error = String(e);
      }
    }
    load();
  }

  const shown = $derived((data?.employees ?? []).filter((e) => statusFilter === 'all' || e.status === statusFilter));
  const visibleRuns = $derived(showAllRuns ? (data?.recentRuns ?? []) : (data?.recentRuns ?? []).slice(0, RECENT_SHOWN));
  const ended = $derived.by(() => {
    const days = data?.runsByDay ?? [];
    const done = days.reduce((n, d) => n + d.done + d.chatTurns, 0);
    const skipped = days.reduce((n, d) => n + d.skipped, 0);
    const waiting = days.reduce((n, d) => n + d.waiting, 0);
    const stopped = days.reduce((n, d) => n + d.stopped, 0);
    const total = done + skipped + waiting + stopped;
    return { done, skipped, waiting, stopped, total, pct: total ? Math.round(((done + skipped) / total) * 100) : 0 };
  });
  const today = $derived(new Date().toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' }));

  // The chart's scale: the top tick is the first round number at or above the
  // busiest day, so the axis reads 0 / 25 / 50 / 75 / 100 rather than 0 / 23 / 46.
  const chartTop = $derived.by(() => {
    const max = Math.max(1, ...(data?.runsByDay ?? []).map((d) => d.done + d.skipped + d.stopped + d.waiting + d.chatTurns));
    const mag = 10 ** Math.floor(Math.log10(max));
    return [1, 2, 4, 5, 10].map((m) => m * mag).find((n) => n >= max) ?? max;
  });
  const ticks = $derived([1, 0.75, 0.5, 0.25, 0].map((f) => Math.round(chartTop * f)));
  function px(v: number) {
    return Math.round((v / chartTop) * BAR_PX) + 'px';
  }
  function dayLabel(day: string) {
    const [, m, d] = day.split('-');
    return `${Number(m)}/${Number(d)}`;
  }
  const statusOptions: Status[] = ['all', 'working', 'waiting', 'idle', 'paused'];

  // ── The shape of the workforce ─────────────────────────────────────────
  // Drawn from the roster the shell already holds: `department` and `reportsTo`
  // ride the same list call every other roster field does, so the shape needs
  // no fetch of its own and no second endpoint that could disagree with the
  // sidebar. Redraws arrive on the roster's own `agent_updated` refresh.

  /** One employee in the tree, with the employees that answer to it. */
  interface OrgNode {
    id: string;
    name: string;
    color: string;
    department: string;
    reportsTo: string;
    depth: number;
    reports: number;
  }

  /** Reporting lines the owner has just changed, until the roster confirms. */
  let pendingLine = $state<Record<string, string>>({});
  let lineSaved = $state('');
  let lineError = $state('');
  let lineTimer: ReturnType<typeof setTimeout> | null = null;

  const orgRoster = $derived(shell.roster.filter((a) => !a.isApp));
  const managerOf = $derived.by(() => {
    const m: Record<string, string> = {};
    for (const a of orgRoster) m[a.id] = pendingLine[a.id] ?? a.reportsTo ?? '';
    // A line that names an employee no longer on the roster reads as "answers
    // to you" — the same way the backend reads a manager id that stopped
    // resolving, so the picture never hides an employee behind a ghost.
    for (const [id, mgr] of Object.entries(m)) {
      if (mgr && !orgRoster.some((a) => a.id === mgr)) m[id] = '';
    }
    return m;
  });

  /** The tree, flattened depth-first: the order it is drawn in. */
  const orgRows = $derived.by(() => {
    const byManager = new Map<string, typeof orgRoster>();
    for (const a of orgRoster) {
      const key = managerOf[a.id] ?? '';
      const list = byManager.get(key) ?? [];
      list.push(a);
      byManager.set(key, list);
    }
    for (const list of byManager.values()) list.sort((x, y) => x.name.localeCompare(y.name));
    const rows: OrgNode[] = [];
    const walk = (managerId: string, depth: number) => {
      for (const a of byManager.get(managerId) ?? []) {
        // Guard against a cycle the server should already have refused: an
        // employee is drawn once, never twice, and never forever.
        if (rows.some((r) => r.id === a.id)) continue;
        rows.push({
          id: a.id,
          name: a.name,
          color: a.color,
          department: (a.department ?? '').trim(),
          reportsTo: managerId,
          depth,
          reports: (byManager.get(a.id) ?? []).length,
        });
        walk(a.id, depth + 1);
      }
    };
    walk('', 0);
    // Anyone a refused cycle left unreachable still belongs on the page.
    for (const a of orgRoster) {
      if (!rows.some((r) => r.id === a.id)) {
        rows.push({
          id: a.id,
          name: a.name,
          color: a.color,
          department: (a.department ?? '').trim(),
          reportsTo: managerOf[a.id] ?? '',
          depth: 0,
          reports: 0,
        });
      }
    }
    return rows;
  });

  /** Everyone flat under you is no hierarchy at all — say so instead of drawing one. */
  const orgIsFlat = $derived(orgRoster.length > 0 && orgRows.every((r) => r.depth === 0));

  /** Who an employee may answer to: not itself, and nobody already below it. */
  function lineOptions(id: string) {
    const below = new Set<string>([id]);
    for (;;) {
      const before = below.size;
      for (const a of orgRoster) {
        const mgr = managerOf[a.id] ?? '';
        if (mgr && below.has(mgr)) below.add(a.id);
      }
      if (below.size === before) break;
    }
    return orgRoster.filter((a) => !below.has(a.id)).sort((x, y) => x.name.localeCompare(y.name));
  }

  /**
   * Set an employee's reporting line. One short debounce and a pill, the same
   * as every other form here — and the same PUT /agents/{id} the employee's own
   * settings uses, so there is one writer for an employee's fields.
   */
  function setReportsTo(id: string, managerId: string) {
    pendingLine = { ...pendingLine, [id]: managerId };
    if (lineTimer) clearTimeout(lineTimer);
    lineTimer = setTimeout(() => saveLine(id, managerId), 700);
  }

  async function saveLine(id: string, managerId: string) {
    try {
      await api.updateAgent(id, { reportsTo: managerId });
      lineError = '';
      lineSaved = id;
      setTimeout(() => { if (lineSaved === id) lineSaved = ''; }, 2000);
    } catch (e) {
      // A refused line names the loop it would have closed. Dropping the
      // pending value puts the picture back to what the server actually holds.
      lineError = (e as Error)?.message || $t('agentSettings.saveFailed');
      const { [id]: _dropped, ...rest } = pendingLine;
      pendingLine = rest;
    }
  }
</script>

<div class="flex-1 flex flex-col min-w-0 min-h-0 w-full max-w-full overflow-x-hidden bg-base-100">
  <div class="flex items-center gap-2.5 h-12 px-3 md:px-5 border-b border-base-300 shrink-0 min-w-0">
    <button class="md:hidden shrink-0 -ml-1 p-1 text-base-content/70" onclick={() => shell?.openList?.()} aria-label={$t('nav.agents')}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 6l-6 6 6 6"/></svg>
    </button>
    <span class="font-semibold text-[15px] shrink-0">{$t('dashboard.title')}</span>
    <span class="text-[13px] text-base-content/55 truncate hidden sm:inline">{today}</span>
    <div class="ml-auto flex items-center gap-1.5 shrink-0">
      <button class="w-8 h-8 rounded-full flex items-center justify-center text-base-content/60 hover:bg-base-200" onclick={() => shell?.openInbox?.()} aria-label={$t('nav.inbox')} title={$t('nav.inbox')}>
        <Bell class="w-[17px] h-[17px]" />
      </button>
      <label class="h-8 pl-2.5 pr-3 rounded-full border border-base-300 flex items-center gap-2 text-xs cursor-pointer select-none">
        <input type="checkbox" class="checkbox checkbox-xs checkbox-primary rounded-full" checked={startHere} onchange={(ev) => setStartPage((ev.currentTarget as HTMLInputElement).checked)} />
        {$t('dashboard.openHere')}
      </label>
    </div>
  </div>

  <div class="flex-1 overflow-auto bg-base-300">
    <!-- One explicit minmax(0,1fr) track: an implicit auto track lets a two-column
         section size itself to its content and run past a phone screen. -->
    <div class="max-w-[1120px] w-full min-w-0 mx-auto px-3 md:px-5 py-3 md:py-5 grid grid-cols-[minmax(0,1fr)] gap-4 md:gap-5">
      {#if error}
        <div class="rounded-2xl border border-error/30 bg-error/5 px-4 py-3 text-sm">{$t('dashboard.loadError')} <span class="text-base-content/60">{error}</span></div>
      {/if}

      {#if data}
        <!-- The story in four numbers: who is working, who needs me, how much
             work happened, how much of it happened without me. -->
        <section class="grid grid-cols-2 lg:grid-cols-4 gap-3 md:gap-4 min-w-0">
          <div class="rounded-2xl border p-4 min-w-0 flex flex-col gap-2.5 bg-base-100 shadow-sm {data.counts.working > 0 ? 'border-success/60' : 'border-base-content/15'}">
            <div class="w-9 h-9 rounded-xl flex items-center justify-center bg-primary/10 text-primary"><Users class="w-[18px] h-[18px]" /></div>
            <div class="flex items-center gap-2">
              <span class="text-3xl font-semibold leading-none tracking-tight tabular-nums">{data.counts.working}</span>
              {#if data.counts.working > 0}<i class="w-2.5 h-2.5 rounded-full bg-success animate-pulse" aria-hidden="true"></i>{/if}
            </div>
            <div>
              <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('dashboard.workingNow')}</div>
              <div class="text-xs text-base-content/50 mt-0.5">{$t('dashboard.ofEmployees', { values: { n: data.counts.employees, paused: data.counts.paused } })}</div>
            </div>
          </div>
          <div class="rounded-2xl border p-4 min-w-0 flex flex-col gap-2.5 bg-base-100 shadow-sm {data.counts.waiting > 0 ? 'border-warning/70' : 'border-base-content/15'}">
            <div class="w-9 h-9 rounded-xl flex items-center justify-center bg-warning/15 text-warning"><Clock class="w-[18px] h-[18px]" /></div>
            <span class="text-3xl font-semibold leading-none tracking-tight tabular-nums">{data.counts.waiting}</span>
            <div>
              <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('dashboard.waitingOnYou')}</div>
              <div class="text-xs text-base-content/50 mt-0.5 truncate">{data.approvals[0] ? `${data.approvals[0].agentName}, ${data.approvals[0].summary}` : $t('dashboard.nothingRightNow')}</div>
            </div>
          </div>
          <div class="rounded-2xl border border-base-content/15 bg-base-100 shadow-sm p-4 min-w-0 flex flex-col gap-2.5">
            <div class="w-9 h-9 rounded-xl flex items-center justify-center bg-success/15 text-success"><TrendingUp class="w-[18px] h-[18px]" /></div>
            <span class="text-3xl font-semibold leading-none tracking-tight tabular-nums">{data.counts.runsToday}</span>
            <div>
              <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('dashboard.runsToday')}</div>
              <div class="text-xs text-base-content/50 mt-0.5">{$t('dashboard.runsTodayDetail', { values: { done: data.counts.doneToday, skipped: data.counts.skippedToday, chats: data.counts.chatTurnsToday, stopped: data.counts.stoppedToday } })}</div>
            </div>
          </div>
          <div class="rounded-2xl border border-base-content/15 bg-base-100 shadow-sm p-4 min-w-0 flex flex-col gap-2.5">
            <div class="w-9 h-9 rounded-xl flex items-center justify-center bg-base-content/8 text-base-content/70"><BadgeCheck class="w-[18px] h-[18px]" /></div>
            <span class="text-3xl font-semibold leading-none tracking-tight tabular-nums">{ended.total ? `${ended.pct}%` : '–'}</span>
            <div>
              <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('dashboard.workedWithoutYou')}</div>
              <div class="text-xs text-base-content/50 mt-0.5">{ended.total ? $t('dashboard.workedWithoutYouSub', { values: { n: ended.done + ended.skipped, total: ended.total } }) : $t('dashboard.noRunsYet')}</div>
            </div>
          </div>
        </section>

        <!-- Needs your okay: the interrupt. Cards stay in place; this is what moves. -->
        {#each data.approvals as a (a.id)}
          <section class="rounded-2xl border border-warning/60 bg-warning/10 px-4 py-3 flex flex-wrap items-center gap-3">
            <div class="flex items-center gap-2.5 min-w-0">
              <AgentAvatar name={a.agentName} color={colors[a.agentId]} size="sm" />
              <p class="m-0 text-[13px]"><b>{$t('dashboard.needsOkay', { values: { name: a.agentName } })}</b> {a.summary} <span class="text-base-content/50">· {formatRelative(a.since * 1000, 'short')}</span></p>
            </div>
            <div class="flex gap-1.5 ml-auto shrink-0">
              <button class="btn btn-primary btn-sm rounded-full" onclick={() => answer(a, true)}>{$t('dashboard.approve')}</button>
              <button class="btn btn-sm rounded-full" onclick={() => answer(a, false)}>{$t('dashboard.decline')}</button>
              {#if a.chatId}<button class="btn btn-ghost btn-sm rounded-full" onclick={() => goto(`/${a.agentId}/threads/${a.chatId}`)}>{$t('dashboard.openChat')}</button>{/if}
            </div>
          </section>
        {/each}

        <!-- Employees -->
        <section class="min-w-0">
          <div class="flex items-center gap-3 mb-3">
            <h2 class="text-[15px] font-semibold">{$t('dashboard.employees')}</h2>
            <div class="ml-auto flex items-center gap-2">
              <div class="h-8 rounded-full border border-base-300 p-0.5 flex" role="group" aria-label={$t('dashboard.employees')}>
                <button class="w-8 h-full rounded-full flex items-center justify-center {view === 'grid' ? 'bg-primary/10 text-primary' : 'text-base-content/50'}" aria-pressed={view === 'grid'} aria-label={$t('dashboard.viewGrid')} title={$t('dashboard.viewGrid')} onclick={() => (view = 'grid')}><LayoutGrid class="w-4 h-4" /></button>
                <button class="w-8 h-full rounded-full flex items-center justify-center {view === 'list' ? 'bg-primary/10 text-primary' : 'text-base-content/50'}" aria-pressed={view === 'list'} aria-label={$t('dashboard.viewList')} title={$t('dashboard.viewList')} onclick={() => (view = 'list')}><List class="w-4 h-4" /></button>
                <button class="w-8 h-full rounded-full flex items-center justify-center {view === 'org' ? 'bg-primary/10 text-primary' : 'text-base-content/50'}" aria-pressed={view === 'org'} aria-label={$t('org.title')} title={$t('org.title')} onclick={() => (view = 'org')}><Network class="w-4 h-4" /></button>
              </div>
              {#if view !== 'org'}
                <select class="select select-sm select-bordered rounded-full h-8 min-h-0 text-xs" bind:value={statusFilter} aria-label={$t('dashboard.allStatus')}>
                  {#each statusOptions as s}
                    <option value={s}>{s === 'all' ? $t('dashboard.allStatus') : $t(`dashboard.status.${s}`)}</option>
                  {/each}
                </select>
              {/if}
            </div>
          </div>

          {#if view === 'org'}
            <!-- The shape of the workforce: who answers to whom, which part of
                 the company each sits in, and who is filed nowhere. Indentation
                 IS the reporting line, so the picture cannot disagree with the
                 data — every row's manager is the row it is indented under. -->
            <div class="rounded-2xl border border-base-content/15 bg-base-100 shadow-sm p-4 min-w-0">
              <div class="text-xs text-base-content/70 mb-3">{$t('org.blurb')}</div>
              {#if lineError}
                <div class="rounded-lg border border-error/40 bg-error/10 px-3.5 py-2.5 text-xs text-error mb-3">{lineError}</div>
              {/if}
              {#if orgRoster.length === 0}
                <div class="text-xs text-base-content/50 py-4">{$t('org.empty')}</div>
              {:else}
                <div class="flex items-center gap-2.5 pb-2 mb-1 border-b border-base-300">
                  <span class="w-7 h-7 rounded-lg grid place-items-center bg-base-content/10 text-base-content/60 shrink-0" aria-hidden="true"><Users class="w-3.5 h-3.5" /></span>
                  <span class="text-sm font-semibold">{$t('org.you')}</span>
                  <span class="text-xs text-base-content/50">{$t('org.reportsCount', { values: { count: orgRows.filter((r) => r.depth === 0).length } })}</span>
                </div>
                {#if orgIsFlat}
                  <div class="text-xs text-base-content/60 pt-2 pb-1">{$t('org.flat')}</div>
                {/if}
                <div class="divide-y divide-base-300/70">
                  {#each orgRows as r (r.id)}
                    <div class="py-2 flex items-stretch gap-3 min-w-0">
                      <!-- One rail per level: the indent IS the reporting line,
                           drawn with borders rather than a picture of a tree. -->
                      {#each Array(r.depth) as _, i (i)}
                        <i class="w-[1.15rem] shrink-0 border-l border-base-300" aria-hidden="true"></i>
                      {/each}
                      <div class="flex items-center gap-2.5 min-w-0 flex-1">
                        <AgentAvatar name={r.name} color={r.color} size="sm" />
                        <button class="text-sm font-semibold truncate link link-hover no-underline text-left" onclick={() => goto(`/${r.id}/threads`)}>{r.name}</button>
                        {#if r.department}
                          <span class="text-[10px] font-semibold uppercase tracking-wider px-2 py-px rounded-full bg-base-content/5 text-base-content/60 shrink-0">{r.department}</span>
                        {:else}
                          <span class="text-[10px] uppercase tracking-wider px-2 py-px rounded-full border border-dashed border-base-300 text-base-content/40 shrink-0">{$t('org.unassigned')}</span>
                        {/if}
                        {#if r.reports > 0}
                          <span class="text-xs text-base-content/45 shrink-0">{$t('org.reportsCount', { values: { count: r.reports } })}</span>
                        {/if}
                      </div>
                      <div class="flex items-center gap-2 shrink-0">
                        {#if lineSaved === r.id}
                          <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> {$t('common.saved')}</span>
                        {/if}
                        <label class="flex items-center gap-1.5">
                          <span class="text-[10px] uppercase tracking-wider text-base-content/45">{$t('agentSettings.reportsTo')}</span>
                          <select
                            class="select select-bordered select-xs rounded-full min-h-0 h-7 text-xs max-w-[11rem]"
                            value={managerOf[r.id] ?? ''}
                            onchange={(e) => setReportsTo(r.id, e.currentTarget.value)}
                          >
                            <option value="">{$t('org.reportsToYou')}</option>
                            {#each lineOptions(r.id) as o (o.id)}
                              <option value={o.id}>{o.name}</option>
                            {/each}
                          </select>
                        </label>
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {:else if view === 'grid'}
            <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3 md:gap-4 min-w-0">
              {#each shown as e (e.id)}
                {@const working = e.status === 'working'}
                <!-- Every card uses the same four slots, so a working card is
                     marked, not taller: name row, what it is on, the live line
                     (or what comes next), the actions. -->
                <div class="rounded-2xl border p-4 grid grid-rows-[auto_1fr_auto_auto] gap-3 min-w-0 shadow-sm bg-base-100 {working ? 'border-success/60' : e.status === 'waiting' ? 'border-warning/70' : 'border-base-content/15'}">
                  <div class="flex items-center gap-3 min-w-0">
                    <AgentAvatar name={e.name} color={colors[e.id]} />
                    <span class="font-semibold text-[15px] truncate">{e.name}</span>
                    <!-- No pill means idle: the pill is reserved for a state worth noticing. -->
                    {#if e.status !== 'idle'}
                      <span class="ml-auto text-[10px] font-semibold uppercase tracking-wider px-2 py-px rounded-full shrink-0 flex items-center gap-1 {working ? 'bg-success text-success-content' : e.status === 'waiting' ? 'bg-warning text-warning-content' : 'bg-base-content/5 text-base-content/55'}">
                        {#if working}<i class="w-1.5 h-1.5 rounded-full bg-success-content animate-pulse" aria-hidden="true"></i>{/if}{$t(`dashboard.status.${e.status}`)}
                      </span>
                    {/if}
                  </div>
                  <div class="min-w-0 grid content-start gap-1.5">
                    {#if working}
                      <div class="text-sm font-medium leading-snug line-clamp-2">{e.task}</div>
                      {#if e.step && e.stepCount}
                        <div class="flex items-center gap-2 text-xs text-base-content/60">
                          <span class="shrink-0">{$t('dashboard.stepsOf', { values: { step: e.step, total: e.stepCount } })}</span>
                          <div class="flex-1 flex gap-0.5" aria-hidden="true">
                            {#each Array(e.stepCount) as _, i}<i class="flex-1 h-1 rounded-full {i < e.step ? 'bg-success' : 'bg-base-300'}"></i>{/each}
                          </div>
                        </div>
                      {/if}
                    {:else if e.lastDetail}
                      <div class="text-sm leading-snug line-clamp-2 {outcomeClass(e.lastOutcome ?? '')}">{e.lastDetail}</div>
                      {#if e.lastRunAt}<div class="text-xs text-base-content/50">{formatRelative(e.lastRunAt * 1000, 'short')}</div>{/if}
                    {:else}
                      <div class="text-sm leading-snug line-clamp-2 text-base-content/70">{#if e.isolated && e.matters > 0}<span class="text-base-content/50">{$t('dashboard.matters', { values: { n: e.matters } })} · </span>{/if}{e.task}</div>
                    {/if}
                  </div>
                  <div class="border-t border-base-300/80 pt-2.5 flex items-center gap-2 text-xs min-w-0 text-base-content/70">
                    {#if working}<i class="w-1.5 h-1.5 rounded-full bg-success shrink-0" aria-hidden="true"></i>{/if}
                    <span class="truncate">{e.activity}</span>
                  </div>
                  <div class="flex items-center gap-3 text-xs">
                    {#if working}
                      <button class="link link-primary no-underline font-medium" onclick={() => openWork(e)}>{e.runId ? $t('dashboard.openRun') : e.isolated ? $t('dashboard.openMatters') : $t('dashboard.openChat')}</button>
                    {:else}
                      <button class="link link-primary no-underline" onclick={() => openChat(e)}>{e.isolated ? $t('dashboard.openMatters') : $t('dashboard.openChat')}</button>
                    {/if}
                    <button class="link link-primary no-underline" onclick={() => openRuns(e.id)}>{$t('dashboard.runs')}</button>
                    {#if e.lastActivityAt}<span class="ml-auto text-base-content/50 font-mono tabular-nums">{formatTime(e.lastActivityAt * 1000)}</span>{/if}
                  </div>
                </div>
              {:else}
                <div class="text-xs text-base-content/50 py-4">{$t('dashboard.noEmployeesMatch')}</div>
              {/each}
            </div>
          {:else}
            <div class="rounded-2xl border border-base-content/15 bg-base-100 shadow-sm divide-y divide-base-300 min-w-0">
              {#each shown as e (e.id)}
                {@const working = e.status === 'working'}
                <!-- Real columns: avatar, name, status, detail, actions. Every
                     row's status and detail start on the same x. -->
                <div class="px-4 py-2.5 grid grid-cols-[36px_minmax(0,1fr)_auto_auto] md:grid-cols-[36px_minmax(0,1.2fr)_5.5rem_minmax(0,2fr)_auto] items-center gap-3 min-w-0 {working ? 'bg-success/5' : ''}">
                  <AgentAvatar name={e.name} color={colors[e.id]} />
                  <span class="font-semibold text-[15px] truncate min-w-0">{e.name}</span>
                  <span class="justify-self-start text-[10px] font-semibold uppercase tracking-wider px-2 py-px rounded-full {working ? 'bg-success text-success-content' : e.status === 'waiting' ? 'bg-warning text-warning-content' : 'bg-base-content/5 text-base-content/55'} {e.status === 'idle' ? 'invisible' : ''}">{$t(`dashboard.status.${e.status}`)}</span>
                  <div class="hidden md:block min-w-0 text-xs truncate text-base-content/70">{working ? `${e.task} · ${e.activity}` : e.lastDetail ? `${e.lastDetail} · ${e.activity}` : e.activity}</div>
                  <div class="grid grid-cols-[6.5rem_auto] items-center gap-3 text-xs shrink-0">
                    {#if working}
                      <button class="link link-primary no-underline font-medium" onclick={() => openWork(e)}>{e.runId ? $t('dashboard.openRun') : $t('dashboard.openChat')}</button>
                    {:else}
                      <button class="link link-primary no-underline" onclick={() => openChat(e)}>{e.isolated ? $t('dashboard.openMatters') : $t('dashboard.openChat')}</button>
                    {/if}
                    <button class="link link-primary no-underline" onclick={() => openRuns(e.id)}>{$t('dashboard.runs')}</button>
                  </div>
                </div>
              {:else}
                <div class="px-4 py-4 text-xs text-base-content/50">{$t('dashboard.noEmployeesMatch')}</div>
              {/each}
            </div>
          {/if}
        </section>

        <!-- How much work happened, and how much of it happened without you -->
        <section class="grid grid-cols-1 md:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] gap-3 md:gap-4 min-w-0">
          <div class="rounded-2xl border border-base-content/15 bg-base-100 shadow-sm p-4 min-w-0">
            <div class="flex items-baseline justify-between gap-3 mb-4">
              <h3 class="text-[14px] font-semibold">{$t('dashboard.activityOverTime')}</h3>
              <span class="text-xs text-base-content/50">{$t('dashboard.last14Days')}</span>
            </div>
            <div class="grid grid-cols-[28px_minmax(0,1fr)] gap-2">
              <div class="flex flex-col justify-between text-[10px] text-base-content/45 text-right tabular-nums leading-none" style:height="{BAR_PX}px">
                {#each ticks as tick}<span>{tick}</span>{/each}
              </div>
              <div class="relative" style:height="{BAR_PX}px">
                {#each [1, 0.75, 0.5, 0.25, 0] as f}
                  <i class="absolute left-0 right-0 border-t border-base-300/70" style:bottom="{f * 100}%" aria-hidden="true"></i>
                {/each}
                <div class="absolute inset-0 grid grid-cols-14 gap-1.5 items-end px-1">
                  {#each data.runsByDay as d (d.day)}
                    <div class="group relative flex flex-col justify-end h-full">
                      <div class="hidden group-hover:block absolute bottom-full left-1/2 -translate-x-1/2 mb-1.5 z-10 whitespace-nowrap rounded-lg bg-base-content text-base-100 text-[11px] leading-snug px-2.5 py-1.5 shadow-lg pointer-events-none">
                        <b>{dayLabel(d.day)}</b> · {$t('dashboard.barHover', { values: { done: d.done + d.chatTurns, skipped: d.skipped, waiting: d.waiting, stopped: d.stopped } })}
                      </div>
                      <i class="block bg-error rounded-t-sm" style:height={px(d.stopped)}></i>
                      <i class="block bg-warning" style:height={px(d.waiting)}></i>
                      <i class="block bg-base-300" style:height={px(d.skipped)}></i>
                      <i class="block bg-success" style:height={px(d.done + d.chatTurns)}></i>
                    </div>
                  {/each}
                </div>
              </div>
              <span></span>
              <div class="flex justify-between text-[10px] text-base-content/45 tabular-nums px-1"><span>{dayLabel(data.runsByDay[0]?.day ?? '')}</span><span>{dayLabel(data.runsByDay[Math.floor(data.runsByDay.length / 2)]?.day ?? '')}</span><span>{dayLabel(data.runsByDay[data.runsByDay.length - 1]?.day ?? '')}</span></div>
            </div>
            <div class="flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-base-content/60 mt-3">
              <span><i class="inline-block w-2 h-2 rounded-full bg-success mr-1"></i>{$t('dashboard.done')}</span>
              <span><i class="inline-block w-2 h-2 rounded-full bg-base-300 mr-1"></i>{$t('dashboard.nothingToDo')}</span>
              <span><i class="inline-block w-2 h-2 rounded-full bg-warning mr-1"></i>{$t('dashboard.neededOkay')}</span>
              <span><i class="inline-block w-2 h-2 rounded-full bg-error mr-1"></i>{$t('dashboard.stopped')}</span>
            </div>
          </div>
          <div class="rounded-2xl border border-base-content/15 bg-base-100 shadow-sm p-4 min-w-0">
            <div class="flex items-baseline justify-between gap-3 mb-4">
              <h3 class="text-[14px] font-semibold">{$t('dashboard.workedWithoutYou')}</h3>
              <span class="text-xs text-base-content/50">{$t('dashboard.last14DaysCount', { values: { n: ended.total } })}</span>
            </div>
            <div class="flex items-center gap-5">
              <svg viewBox="0 0 36 36" class="w-24 h-24 shrink-0">
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-base-300" stroke-width="3.5"/>
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-success" stroke-width="3.5" stroke-linecap="round" stroke-dasharray="{ended.total ? ((ended.done + ended.skipped) / ended.total) * 100 : 0} 100" transform="rotate(-90 18 18)"/>
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-warning" stroke-width="3.5" stroke-dasharray="{ended.total ? (ended.waiting / ended.total) * 100 : 0} 100" stroke-dashoffset="-{ended.total ? ((ended.done + ended.skipped) / ended.total) * 100 : 0}" transform="rotate(-90 18 18)"/>
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-error" stroke-width="3.5" stroke-dasharray="{ended.total ? (ended.stopped / ended.total) * 100 : 0} 100" stroke-dashoffset="-{ended.total ? ((ended.done + ended.skipped + ended.waiting) / ended.total) * 100 : 0}" transform="rotate(-90 18 18)"/>
                <text x="18" y="19.5" text-anchor="middle" font-size="7.5" font-weight="600" class="fill-current">{ended.total ? `${ended.pct}%` : '–'}</text>
              </svg>
              <div class="grid gap-1.5 text-xs min-w-0">
                <div class="text-[13px] leading-snug mb-1">{$t('dashboard.workedWithoutYouLine', { values: { pct: ended.pct } })}</div>
                <div><b class="tabular-nums">{ended.done}</b> <span class="text-base-content/60">{$t('dashboard.endedDone')}</span></div>
                <div><b class="tabular-nums">{ended.skipped}</b> <span class="text-base-content/60">{$t('dashboard.endedSkipped')}</span></div>
                <div><b class="tabular-nums">{ended.waiting}</b> <span class="text-base-content/60">{$t('dashboard.endedWaited')}</span></div>
                <div><b class="tabular-nums">{ended.stopped}</b> <span class="text-base-content/60">{$t('dashboard.endedStopped')}</span></div>
              </div>
            </div>
          </div>
        </section>

        <!-- Recent runs: the last few. The full audit view is one click away. -->
        <section class="min-w-0">
          <div class="flex items-baseline gap-3 mb-3">
            <h2 class="text-[15px] font-semibold">{$t('dashboard.recentRuns')}</h2>
            {#if data.recentRuns.length > RECENT_SHOWN}
              <button class="ml-auto link link-primary no-underline text-xs" onclick={() => (showAllRuns = !showAllRuns)}>{showAllRuns ? $t('dashboard.showFewer') : $t('dashboard.showAll', { values: { n: data.recentRuns.length } })}</button>
            {/if}
          </div>
          <!-- Phone: one stacked row per run, nothing scrolls sideways. -->
          <ul class="md:hidden divide-y divide-base-300 border-y border-base-300 -mx-3 px-3">
            {#each visibleRuns as r (r.id)}
              <li class="py-2.5 grid gap-1 min-w-0">
                <div class="flex items-center gap-2 min-w-0">
                  <AgentAvatar name={r.agentName} color={colors[r.agentId]} size="xs" />
                  <span class="text-[13px] font-medium truncate">{r.agentName}</span>
                  <span class="ml-auto text-[11px] text-base-content/50 whitespace-nowrap tabular-nums">{formatTime(r.startedAt * 1000)}</span>
                </div>
                <div class="text-[13px] truncate text-base-content/80">{r.title}</div>
                <div class="flex items-start gap-3 min-w-0">
                  <span class="text-xs font-medium leading-snug min-w-0 flex-1 {outcomeClass(r.outcome)}">{r.detail}</span>
                  <button class="link link-primary no-underline text-xs shrink-0" onclick={() => openRuns(r.agentId)}>{$t('dashboard.runs')}</button>
                </div>
              </li>
            {:else}
              <li class="py-3 text-xs text-base-content/50">{$t('dashboard.noRunsYet')}</li>
            {/each}
          </ul>
          <div class="hidden md:block rounded-2xl border border-base-content/15 bg-base-100 shadow-sm overflow-x-auto min-w-0">
            <table class="w-full min-w-[640px] border-collapse text-[13px]">
              <thead>
                <tr class="text-left text-[11px] font-semibold uppercase tracking-wider text-base-content/50">
                  <th class="py-2.5 pl-4 pr-2 border-b border-base-300">{$t('dashboard.colTime')}</th>
                  <th class="py-2.5 pr-2 border-b border-base-300">{$t('dashboard.colEmployee')}</th>
                  <th class="py-2.5 pr-2 border-b border-base-300">{$t('dashboard.colWhat')}</th>
                  <th class="py-2.5 pr-2 border-b border-base-300">{$t('dashboard.colEnded')}</th>
                  <th class="py-2.5 pr-4 border-b border-base-300"></th>
                </tr>
              </thead>
              <tbody>
                {#each visibleRuns as r, i (r.id)}
                  {@const last = i === visibleRuns.length - 1}
                  <tr>
                    <td class="py-2.5 pl-4 pr-2 text-xs text-base-content/60 whitespace-nowrap tabular-nums {last ? '' : 'border-b border-base-300'}">{formatTime(r.startedAt * 1000)}</td>
                    <td class="py-2.5 pr-2 {last ? '' : 'border-b border-base-300'}"><div class="flex items-center gap-2"><AgentAvatar name={r.agentName} color={colors[r.agentId]} size="xs" /><span class="truncate">{r.agentName}</span></div></td>
                    <td class="py-2.5 pr-2 truncate max-w-[280px] {last ? '' : 'border-b border-base-300'}">{r.title}</td>
                    <td class="py-2.5 pr-2 text-xs font-medium {outcomeClass(r.outcome)} {last ? '' : 'border-b border-base-300'}">{r.detail}</td>
                    <td class="py-2.5 pr-4 text-right {last ? '' : 'border-b border-base-300'}"><button class="link link-primary no-underline text-xs" onclick={() => openRuns(r.agentId)}>{$t('dashboard.runs')}</button></td>
                  </tr>
                {:else}
                  <tr><td colspan="5" class="py-3 px-4 text-xs text-base-content/50">{$t('dashboard.noRunsYet')}</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
        </section>
      {:else if !error}
        <div class="py-10 flex justify-center"><span class="loading loading-spinner loading-md"></span></div>
      {/if}
    </div>
  </div>
</div>
