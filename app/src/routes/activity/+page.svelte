<!--
  Activity: every action an employee took, filterable by employee, where it
  started and what was decided. Each row answers "Why was this allowed?" in
  plain words the server renders, with who decided it (the rules, Jev or
  the backup reviewer) and the reviewer's verdict; actions the permission
  check couldn't review are flagged. `?agent=<id>` opens it on one employee.
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { page as appPage } from '$app/stores';
  import { t } from 'svelte-i18n';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import * as api from '$lib/api/nebo';
  import type { ActivityRow } from '$lib/api/nebo';

  const PAGE = 50;
  const doors = ['chat', 'helper', 'workflow', 'schedule', 'heartbeat', 'coworker', 'voice', 'mcp', 'local_api'];
  const decisions = ['allow', 'ask', 'deny'];

  let employees = $state<{ id: string; name: string }[]>([]);
  let agentId = $state(untrack(() => $appPage.url.searchParams.get('agent') ?? ''));
  let door = $state('');
  let decision = $state('');
  let rows = $state<ActivityRow[]>([]);
  let total = $state(0);
  let loading = $state(true);
  let error = $state('');
  let open = $state<Record<number, boolean>>({});

  async function load(reset: boolean) {
    loading = true;
    error = '';
    try {
      const offset = reset ? 0 : rows.length;
      const resp = await api.listPermissionActivity(agentId || undefined, door || undefined, decision || undefined, PAGE, offset);
      rows = reset ? resp.rows : [...rows, ...resp.rows];
      total = resp.total;
      if (reset) open = {};
    } catch {
      error = $t('permissions.activityLoadError');
    } finally {
      loading = false;
    }
  }

  onMount(async () => {
    void load(true);
    try {
      const resp = await api.listAgents(200, 0);
      employees = resp.agents.map((a) => ({ id: a.id, name: a.displayName || a.name }));
    } catch {
      employees = [];
    }
  });

  function whyLabel(d: string): string {
    if (d === 'ask') return $t('permissions.whyAsked');
    if (d === 'deny') return $t('permissions.whyRefused');
    return $t('permissions.whyAllowed');
  }

  function when(at: number): string {
    return new Date(at * 1000).toLocaleString();
  }
</script>

<svelte:head><title>{$t('activity.pageTitle')}</title></svelte:head>

<div class="flex h-screen bg-base-100 text-base-content text-sm">
  <Sidebar activePage="chat" />
  <div class="flex-1 flex flex-col min-w-0 min-h-0">
    <div class="h-12 px-5 border-b border-base-content/10 flex items-center gap-3.5 shrink-0">
      <span class="text-sm font-semibold">{$t('activity.title')}</span>
    </div>

    <div class="flex-1 overflow-auto p-6 max-md:p-4">
      <div class="max-w-[800px]">
        <h1 class="text-xl font-bold tracking-tight mb-1">{$t('activity.title')}</h1>
        <p class="text-xs text-base-content/70 mb-4">{$t('permissions.activityDescription')}</p>

        <div class="flex flex-wrap gap-2 mb-4">
          <select class="select select-sm select-bordered w-auto" bind:value={agentId} onchange={() => load(true)} aria-label={$t('permissions.filterEmployee')}>
            <option value="">{$t('permissions.everyone')}</option>
            {#each employees as e (e.id)}
              <option value={e.id}>{e.name}</option>
            {/each}
          </select>
          <select class="select select-sm select-bordered w-auto" bind:value={door} onchange={() => load(true)} aria-label={$t('permissions.filterDoor')}>
            <option value="">{$t('permissions.allDoors')}</option>
            {#each doors as d (d)}
              <option value={d}>{$t(`permissions.doors.${d}`)}</option>
            {/each}
          </select>
          <select class="select select-sm select-bordered w-auto" bind:value={decision} onchange={() => load(true)} aria-label={$t('permissions.filterDecision')}>
            <option value="">{$t('permissions.allDecisions')}</option>
            {#each decisions as d (d)}
              <option value={d}>{$t(`permissions.decisions.${d}`)}</option>
            {/each}
          </select>
        </div>

        {#if error}
          <div class="alert alert-error mb-4 py-2 text-xs">
            <AlertTriangle class="w-4 h-4 shrink-0" />
            <span>{error}</span>
          </div>
        {/if}

        {#if loading && rows.length === 0}
          <div class="py-6 flex justify-center"><Spinner /></div>
        {:else if rows.length === 0}
          <p class="text-xs text-base-content/60">{$t('permissions.activityEmpty')}</p>
        {:else}
          <ul class="flex flex-col gap-1.5">
            {#each rows as row, i (i)}
              <li class="rounded-lg border border-base-content/10 bg-base-100 px-3.5 py-2.5">
                <div class="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span class="font-medium">{row.employee}</span>
                  <span class="flex-1 min-w-0">{row.action}</span>
                  <span class="badge badge-sm {row.decision === 'deny' ? 'badge-error' : row.decision === 'ask' ? 'badge-warning' : 'badge-ghost'}">{$t(`permissions.decisions.${row.decision}`)}</span>
                  {#if row.unreviewed}
                    <span class="badge badge-sm badge-warning badge-outline">{$t('permissions.unreviewed')}</span>
                  {/if}
                </div>
                <div class="flex flex-wrap items-center gap-x-3 mt-1 text-xs text-base-content/60">
                  <span>{when(row.at)}</span>
                  <span>{$t(`permissions.doors.${row.door}`)}</span>
                  <button type="button" class="link link-hover" aria-expanded={!!open[i]} onclick={() => (open[i] = !open[i])}>{whyLabel(row.decision)}</button>
                </div>
                {#if open[i]}
                  <p class="text-xs mt-1.5">{row.why}</p>
                  <p class="text-xs mt-1 text-base-content/60">{row.decidedBy}</p>
                  {#if row.verdict}
                    <p class="text-xs text-base-content/60">{row.verdict}</p>
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
          {#if rows.length < total}
            <button type="button" class="btn btn-sm btn-ghost mt-3" disabled={loading} onclick={() => load(false)}>{$t('permissions.loadMore')}</button>
          {/if}
        {/if}
      </div>
    </div>
  </div>
</div>
