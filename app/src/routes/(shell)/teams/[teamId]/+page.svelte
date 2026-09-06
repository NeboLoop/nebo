<!--
  A team's conversation, in the main pane — the same seat as an employee's
  chat, not a shelf over it. The sidebar's roster and team list come through
  the shell context so colors and names match the rows; a team the list has
  not learned yet (a fresh link) is fetched once.
-->
<script lang="ts">
  import { getContext } from 'svelte';
  import { page } from '$app/stores';
  import { t } from 'svelte-i18n';
  import Bell from 'lucide-svelte/icons/bell';
  import * as api from '$lib/api/nebo';
  import type { Team } from '$lib/api/neboComponents';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import TeamView from '$lib/components/teams/TeamView.svelte';

  const shell = getContext<AgentPageContext>('agentPage');
  const teamId = $derived($page.params.teamId ?? '');

  let fetched = $state<Team | null>(null);
  let missing = $state(false);
  const team = $derived(
    shell?.teams?.find((x) => x.id === teamId) ?? (fetched?.id === teamId ? fetched : null)
  );

  $effect(() => {
    const id = teamId;
    if (!id || shell?.teams?.some((x) => x.id === id)) return;
    missing = false;
    api
      .listTeams()
      .then((r) => {
        const found = r?.teams?.find((x) => x.id === id) ?? null;
        fetched = found;
        missing = !found;
      })
      .catch(() => {
        missing = true;
      });
  });

  const roster = $derived(
    (shell?.roster ?? []).map((a) => ({
      id: a.id,
      name: a.name,
      initial: a.initial,
      color: a.color,
      loopAgentId: a.loopAgentId,
      isApp: a.isApp,
    }))
  );
</script>

<svelte:head><title>{team?.name ?? $t('teams.section')} - Nebo</title></svelte:head>

<div class="flex-1 flex flex-col min-w-0 min-h-0 w-full max-w-full overflow-x-hidden bg-base-100">
  <div class="flex items-center gap-2.5 h-12 px-3 md:px-5 border-b border-base-300 shrink-0 min-w-0">
    <button class="md:hidden shrink-0 -ml-1 p-1 text-base-content/70" onclick={() => shell?.openList?.()} aria-label={$t('nav.agents')}>
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 6l-6 6 6 6"/></svg>
    </button>
    <span class="font-semibold text-[15px] truncate min-w-0">{team?.name ?? ''}</span>
    {#if team}
      <span class="text-[13px] text-base-content/55 shrink-0 hidden sm:inline">{$t('teams.membersCount', { values: { count: team.memberAgentIds.length } })}</span>
    {/if}
    <div class="ml-auto flex items-center gap-1.5 shrink-0">
      <button class="w-8 h-8 rounded-full flex items-center justify-center text-base-content/60 hover:bg-base-200" onclick={() => shell?.openInbox?.()} aria-label={$t('nav.inbox')} title={$t('nav.inbox')}>
        <Bell class="w-[17px] h-[17px]" />
      </button>
    </div>
  </div>

  {#if team}
    {#key team.id}
      <TeamView {team} {roster} />
    {/key}
  {:else if missing}
    <div class="flex-1 flex flex-col items-center justify-center text-center px-6">
      <p class="text-sm font-medium">{$t('teams.gone')}</p>
    </div>
  {:else}
    <div class="flex-1 flex items-center justify-center">
      <span class="loading loading-spinner loading-md text-primary"></span>
    </div>
  {/if}
</div>
