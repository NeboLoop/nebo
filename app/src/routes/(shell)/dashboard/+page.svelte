<!--
  The Dashboard: the whole workforce on one page. Everything here is read
  from GET /api/v1/dashboard, which the server composes from what it already
  tracks (the run registry, pending approvals, workflow runs, chat history,
  schedules). Chat stays where it is; each card links back into it.
-->
<script lang="ts">
  import { onMount, onDestroy, getContext } from 'svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import { t } from 'svelte-i18n';
  import { goto } from '$lib/nav';
  import * as api from '$lib/api/nebo';
  import type * as components from '$lib/api/neboComponents';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { AGENT_COLORS_MAP, assignAgentColors } from '$lib/tokens';
  import { formatTime, formatRelative } from '$lib/time';

  // The phone has no sidebar on screen: the header's back chevron opens the
  // employee list the way every thread page does.
  const shell = getContext<AgentPageContext>('agentPage');

  /** How often the page re-reads when no event has arrived. */
  const REFRESH_MS = 15_000;
  /** Height of the tallest bar in the runs chart. */
  const BAR_PX = 96;

  let data = $state<components.DashboardResponse | null>(null);
  let plan = $state<{ plan: string; renewsAt: string | null } | null>(null);
  // Where Nebo opens: a user preference, so the phone and the desktop agree.
  let startHere = $state(false);
  let error = $state('');
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

  async function loadPlan() {
    try {
      const resp = (await api.neboAIBillingSubscription()) as { plan?: string; subscriptions?: Array<{ currentPeriodEnd?: string | null }> };
      plan = { plan: resp.plan ?? '', renewsAt: resp.subscriptions?.[0]?.currentPeriodEnd ?? null };
    } catch {
      plan = null;
    }
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
    loadPlan();
    loadStartPage();
    timer = setInterval(load, REFRESH_MS);
    const ws = getWebSocketClient();
    // Anything that changes a card or a count: a run starting or ending, an
    // approval asked or answered, an employee paused or resumed.
    for (const ev of ['chat_start', 'chat_complete', 'chat_error', 'approval_request', 'nebo:agent_activated', 'nebo:agent_deactivated', 'workflow_run_started', 'workflow_run_completed', 'workflow_run_failed', 'workflow_run_exited']) {
      unsubs.push(ws.on(ev, () => { load(); }));
    }
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
    for (const off of unsubs) off();
  });

  // The same assignment the sidebar makes (user-set colour first, then a
  // stable fallback per employee), so a card matches its row.
  const colors = $derived(assignAgentColors(data?.employees ?? []));
  function colorOf(agentId: string) {
    return AGENT_COLORS_MAP[colors[agentId] ?? ''] ?? AGENT_COLORS_MAP['teal'];
  }
  function initialOf(name: string) {
    return (name.trim()[0] ?? '?').toUpperCase();
  }
  function openChat(e: components.DashboardEmployee) {
    // An isolated employee keeps one thread per matter: open its list, the
    // same screen the sidebar's chevron opens.
    if (e.isolated) return goto(`/${e.id}/threads?list=${e.id}`);
    goto(e.chatId ? `/${e.id}/threads/${e.chatId}` : `/${e.id}/threads`);
  }
  function openRuns(agentId: string) {
    // The runs panel is a shelf over the employee's threads.
    goto(`/${agentId}/threads?runs=1`);
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

  const renewsIn = $derived.by(() => {
    if (!plan?.renewsAt) return '';
    const days = Math.max(0, Math.round((new Date(plan.renewsAt).getTime() - Date.now()) / 86_400_000));
    return $t('dashboard.renewsIn', { values: { days } });
  });
  const maxDay = $derived(Math.max(1, ...(data?.runsByDay ?? []).map((d) => d.done + d.skipped + d.stopped + d.waiting + d.chatTurns)));
  const maxByEmployee = $derived(Math.max(1, ...(data?.runsByEmployee ?? []).map((r) => r.runs)));
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
  function px(v: number, max: number) {
    return Math.round((v / max) * BAR_PX) + 'px';
  }
  function dayLabel(day: string) {
    const [, m, d] = day.split('-');
    return `${Number(m)}/${Number(d)}`;
  }
</script>

<div class="flex-1 flex flex-col min-w-0 min-h-0 w-full max-w-full overflow-x-hidden bg-base-100">
  <div class="flex items-center gap-2.5 h-11 px-3 md:px-5 border-b border-base-300 shrink-0 min-w-0">
    <button class="md:hidden shrink-0 -ml-1 p-1 text-base-content/70" onclick={() => shell?.openList?.()} aria-label={$t('nav.agents')}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 6l-6 6 6 6"/></svg>
    </button>
    <span class="font-semibold text-sm shrink-0">{$t('dashboard.title')}</span>
    <span class="text-[13px] text-base-content/60 truncate hidden sm:inline">{today}</span>
    <label class="ml-auto flex items-center gap-2 text-xs text-base-content/60 cursor-pointer shrink-0">
      <input type="checkbox" class="checkbox checkbox-xs" checked={startHere} onchange={(ev) => setStartPage((ev.currentTarget as HTMLInputElement).checked)} />
      {$t('dashboard.openHere')}
    </label>
  </div>

  <div class="flex-1 overflow-auto">
    <!-- One explicit minmax(0,1fr) track: an implicit auto track lets a two-column
         section size itself to its content and run past a phone screen. -->
    <div class="max-w-[1120px] w-full min-w-0 mx-auto px-3 md:px-5 py-3 md:py-4 grid grid-cols-[minmax(0,1fr)] gap-3 md:gap-4">
      {#if error}
        <div class="rounded-box border border-error/30 bg-error/5 px-4 py-3 text-sm">{$t('dashboard.loadError')} <span class="text-base-content/60">{error}</span></div>
      {/if}

      {#if data}
        <!-- Counts -->
        <section class="grid grid-cols-2 lg:grid-cols-4 gap-2 md:gap-3 min-w-0">
          <div class="rounded-box border border-base-300 px-3 py-3 md:px-4 md:py-3.5 min-w-0">
            <div class="text-[22px] md:text-[26px] font-semibold leading-tight tracking-tight tabular-nums">{data.counts.working}</div>
            <div class="text-[13px] font-medium mt-1">{$t('dashboard.workingNow')}</div>
            <div class="text-xs text-base-content/50 mt-0.5">{$t('dashboard.ofEmployees', { values: { n: data.counts.employees, paused: data.counts.paused } })}</div>
          </div>
          <div class="rounded-box border border-base-300 px-3 py-3 md:px-4 md:py-3.5 min-w-0">
            <div class="text-[22px] md:text-[26px] font-semibold leading-tight tracking-tight tabular-nums">{data.counts.waiting}</div>
            <div class="text-[13px] font-medium mt-1">{$t('dashboard.waitingOnYou')}</div>
            <div class="text-xs text-base-content/50 mt-0.5 truncate">{data.approvals[0] ? `${data.approvals[0].agentName}, ${data.approvals[0].summary}` : $t('dashboard.nothingWaiting')}</div>
          </div>
          <div class="rounded-box border border-base-300 px-3 py-3 md:px-4 md:py-3.5 min-w-0">
            <div class="text-[22px] md:text-[26px] font-semibold leading-tight tracking-tight tabular-nums">{data.counts.runsToday}</div>
            <div class="text-[13px] font-medium mt-1">{$t('dashboard.runsToday')}</div>
            <div class="text-xs text-base-content/50 mt-0.5">{$t('dashboard.runsTodayDetail', { values: { done: data.counts.doneToday, skipped: data.counts.skippedToday, chats: data.counts.chatTurnsToday, stopped: data.counts.stoppedToday } })}</div>
          </div>
          <div class="rounded-box border border-base-300 px-3 py-3 md:px-4 md:py-3.5 min-w-0">
            <div class="text-[22px] md:text-[26px] font-semibold leading-tight tracking-tight capitalize truncate">{plan?.plan || $t('dashboard.planUnknown')}</div>
            <div class="text-[13px] font-medium mt-1">{$t('dashboard.plan')}</div>
            <div class="text-xs text-base-content/50 mt-0.5">{renewsIn || $t('dashboard.workIncluded')}</div>
          </div>
        </section>

        <!-- Needs your okay -->
        {#each data.approvals as a (a.id)}
          {@const ac = colorOf(a.agentId)}
          <section class="rounded-box border border-primary/30 bg-primary/10 px-4 py-3 flex flex-wrap items-center gap-3">
            <div class="flex items-center gap-2.5 min-w-0">
              <div class="w-7 h-7 rounded-field flex items-center justify-center font-mono text-xs font-semibold shrink-0 {ac.bgClass} {ac.inkClass}">{initialOf(a.agentName)}</div>
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
        <section>
          <div class="flex items-baseline gap-2.5 mb-2">
            <h2 class="text-[13px] font-semibold uppercase tracking-wider text-base-content/50">{$t('dashboard.employees')}</h2>
          </div>
          <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3 min-w-0">
            {#each data.employees as e (e.id)}
              {@const ac = colorOf(e.id)}
              <div class="rounded-box border px-3.5 py-3 grid gap-2.5 content-start min-w-0 {e.status === 'working' ? 'border-primary/30 bg-primary/5' : 'border-base-300'}">
                <div class="flex items-center gap-2.5">
                  <div class="w-[26px] h-[26px] rounded-field flex items-center justify-center font-mono text-xs font-semibold shrink-0 {ac.bgClass} {ac.inkClass}">{initialOf(e.name)}</div>
                  <span class="font-medium text-sm truncate">{e.name}</span>
                  <span class="ml-auto text-[10px] font-semibold uppercase tracking-wider px-2 py-px rounded-full shrink-0 {e.status === 'working' ? 'bg-warning/15 text-warning' : e.status === 'waiting' ? 'bg-primary/15 text-primary' : 'bg-base-content/5 text-base-content/60'}">{$t(`dashboard.status.${e.status}`)}</span>
                </div>
                <div class="text-[13px] px-2.5 py-2 rounded-field border border-base-300 bg-base-100 truncate {e.status === 'working' ? 'text-primary' : ''}">{#if e.isolated && e.matters > 0}<span class="text-base-content/60">{$t('dashboard.matters', { values: { n: e.matters } })} · </span>{/if}{e.task}</div>
                <div class="flex items-center gap-2 text-xs text-base-content/60">
                  {#if e.status === 'working'}<span class="loading loading-spinner loading-xs text-warning"></span>{/if}
                  <span class="truncate">{e.activity}</span>
                </div>
                <div class="flex gap-3 text-xs">
                  <button class="link link-primary no-underline" onclick={() => openChat(e)}>{e.isolated ? $t('dashboard.openMatters') : $t('dashboard.openChat')}</button>
                  <button class="link link-primary no-underline" onclick={() => openRuns(e.id)}>{$t('dashboard.runs')}</button>
                  {#if e.lastActivityAt}<span class="ml-auto text-base-content/50 font-mono">{formatTime(e.lastActivityAt * 1000)}</span>{/if}
                </div>
              </div>
            {/each}
          </div>
        </section>

        <!-- Charts -->
        <section class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-3 min-w-0">
          <div class="rounded-box border border-base-300 px-3.5 py-3 min-w-0">
            <h3 class="text-[13px] font-semibold">{$t('dashboard.runsChart')}</h3>
            <div class="text-xs text-base-content/50 mb-2.5">{$t('dashboard.last14Days')}</div>
            <div class="grid grid-cols-14 gap-1 items-end" style:height="{BAR_PX}px">
              {#each data.runsByDay as d (d.day)}
                <div class="flex flex-col justify-end h-full" title="{dayLabel(d.day)}: {d.done + d.chatTurns} done, {d.skipped} nothing to do, {d.waiting} waited, {d.stopped} stopped">
                  <i class="block bg-error rounded-t-sm" style:height={px(d.stopped, maxDay)}></i>
                  <i class="block bg-primary" style:height={px(d.waiting, maxDay)}></i>
                  <i class="block bg-base-300" style:height={px(d.skipped, maxDay)}></i>
                  <i class="block bg-success" style:height={px(d.done + d.chatTurns, maxDay)}></i>
                </div>
              {/each}
            </div>
            <div class="flex justify-between text-[10px] text-base-content/40 font-mono mt-1"><span>{dayLabel(data.runsByDay[0]?.day ?? '')}</span><span>{dayLabel(data.runsByDay[Math.floor(data.runsByDay.length / 2)]?.day ?? '')}</span><span>{dayLabel(data.runsByDay[data.runsByDay.length - 1]?.day ?? '')}</span></div>
            <div class="flex gap-3 text-[11px] text-base-content/60 mt-2">
              <span><i class="inline-block w-2 h-2 rounded-full bg-success mr-1"></i>{$t('dashboard.done')}</span>
              <span><i class="inline-block w-2 h-2 rounded-full bg-base-300 mr-1"></i>{$t('dashboard.nothingToDo')}</span>
              <span><i class="inline-block w-2 h-2 rounded-full bg-primary mr-1"></i>{$t('dashboard.neededOkay')}</span>
              <span><i class="inline-block w-2 h-2 rounded-full bg-error mr-1"></i>{$t('dashboard.stopped')}</span>
            </div>
          </div>
          <div class="rounded-box border border-base-300 px-3.5 py-3 min-w-0">
            <h3 class="text-[13px] font-semibold">{$t('dashboard.runsByEmployee')}</h3>
            <div class="text-xs text-base-content/50 mb-2.5">{$t('dashboard.last14Days')}</div>
            <div class="grid gap-[7px]">
              {#each data.runsByEmployee.slice(0, 8) as r (r.agentId)}
                <div class="grid grid-cols-[110px_1fr_32px] items-center gap-2 text-xs">
                  <span class="truncate">{r.agentName}</span>
                  <div class="h-2 rounded-full bg-base-300 overflow-hidden"><i class="block h-full bg-primary" style:width="{(r.runs / maxByEmployee) * 100}%"></i></div>
                  <span class="text-right text-base-content/60 font-mono text-[11px]">{r.runs}</span>
                </div>
              {:else}
                <div class="text-xs text-base-content/50">{$t('dashboard.noRunsYet')}</div>
              {/each}
            </div>
          </div>
          <div class="rounded-box border border-base-300 px-3.5 py-3 min-w-0">
            <h3 class="text-[13px] font-semibold">{$t('dashboard.howRunsEnded')}</h3>
            <div class="text-xs text-base-content/50 mb-2.5">{$t('dashboard.last14DaysCount', { values: { n: ended.total } })}</div>
            <div class="flex items-center gap-4">
              <svg viewBox="0 0 36 36" class="w-24 h-24 shrink-0">
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-base-300" stroke-width="4"/>
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-success" stroke-width="4" stroke-dasharray="{ended.total ? ((ended.done + ended.skipped) / ended.total) * 100 : 0} 100" transform="rotate(-90 18 18)"/>
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-primary" stroke-width="4" stroke-dasharray="{ended.total ? (ended.waiting / ended.total) * 100 : 0} 100" stroke-dashoffset="-{ended.total ? ((ended.done + ended.skipped) / ended.total) * 100 : 0}" transform="rotate(-90 18 18)"/>
                <circle cx="18" cy="18" r="15.9" fill="none" class="stroke-error" stroke-width="4" stroke-dasharray="{ended.total ? (ended.stopped / ended.total) * 100 : 0} 100" stroke-dashoffset="-{ended.total ? ((ended.done + ended.skipped + ended.waiting) / ended.total) * 100 : 0}" transform="rotate(-90 18 18)"/>
                <text x="18" y="19.5" text-anchor="middle" font-size="7" font-weight="600" class="fill-current">{ended.pct}%</text>
              </svg>
              <div class="grid gap-1.5 text-xs">
                <div><b class="tabular-nums">{ended.done}</b> <span class="text-base-content/60">{$t('dashboard.endedDone')}</span></div>
                <div><b class="tabular-nums">{ended.skipped}</b> <span class="text-base-content/60">{$t('dashboard.endedSkipped')}</span></div>
                <div><b class="tabular-nums">{ended.waiting}</b> <span class="text-base-content/60">{$t('dashboard.endedWaited')}</span></div>
                <div><b class="tabular-nums">{ended.stopped}</b> <span class="text-base-content/60">{$t('dashboard.endedStopped')}</span></div>
              </div>
            </div>
          </div>
        </section>

        <!-- Recent runs -->
        <section>
          <div class="flex items-baseline gap-2.5 mb-2">
            <h2 class="text-[13px] font-semibold uppercase tracking-wider text-base-content/50">{$t('dashboard.recentRuns')}</h2>
          </div>
          <div class="overflow-x-auto min-w-0 -mx-3 px-3 md:mx-0 md:px-0">
            <table class="w-full min-w-[640px] border-collapse text-[13px]">
              <thead>
                <tr class="text-left text-[11px] font-semibold uppercase tracking-wider text-base-content/50">
                  <th class="py-2 pr-2 border-b border-base-300">{$t('dashboard.colTime')}</th>
                  <th class="py-2 pr-2 border-b border-base-300">{$t('dashboard.colEmployee')}</th>
                  <th class="py-2 pr-2 border-b border-base-300">{$t('dashboard.colWhat')}</th>
                  <th class="py-2 pr-2 border-b border-base-300">{$t('dashboard.colEnded')}</th>
                  <th class="py-2 border-b border-base-300"></th>
                </tr>
              </thead>
              <tbody>
                {#each data.recentRuns as r (r.id)}
                  {@const ac = colorOf(r.agentId)}
                  <tr>
                    <td class="py-2 pr-2 border-b border-base-300 font-mono text-xs text-base-content/60 whitespace-nowrap">{formatTime(r.startedAt * 1000)}</td>
                    <td class="py-2 pr-2 border-b border-base-300"><div class="flex items-center gap-2"><div class="w-[22px] h-[22px] rounded-[6px] flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {ac.bgClass} {ac.inkClass}">{initialOf(r.agentName)}</div><span class="truncate">{r.agentName}</span></div></td>
                    <td class="py-2 pr-2 border-b border-base-300 truncate max-w-[280px]">{r.title}</td>
                    <td class="py-2 pr-2 border-b border-base-300 text-xs font-medium {r.outcome === 'done' ? 'text-success' : r.outcome === 'stopped' ? 'text-error' : r.outcome === 'waiting' ? 'text-primary' : r.outcome === 'skipped' ? 'text-base-content/60' : 'text-warning'}">{r.detail}</td>
                    <td class="py-2 border-b border-base-300 text-right"><button class="link link-primary no-underline text-xs" onclick={() => openRuns(r.agentId)}>{$t('dashboard.runs')}</button></td>
                  </tr>
                {:else}
                  <tr><td colspan="5" class="py-3 text-xs text-base-content/50">{$t('dashboard.noRunsYet')}</td></tr>
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
