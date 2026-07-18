<script lang="ts">
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import { launchApp } from '$lib/apps/launcher';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const agentStatusVal = $derived(ctx.agentStatus(ctx.agentId));
</script>

<!-- Column 2: Overview nav -->
<div class="w-[260px] min-w-[260px] border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatusVal} isApp={true} />

  <div class="flex-1 overflow-y-auto">
    <div class="p-1.5 flex flex-col gap-0.5">
      <a
        href="/{agentId}/overview"
        class="flex items-center w-full text-left py-1.5 px-2.5 rounded-md text-sm cursor-pointer bg-base-100 border border-base-300 shadow-sm font-medium no-underline text-base-content"
      >{$t('agentActivity.overview')}</a>
    </div>
  </div>
</div>

<!-- Column 3: App landing -->
<div class="flex-1 flex flex-col items-center justify-center bg-base-100 min-w-0 gap-4">
  <div class="w-16 h-16 rounded-xl bg-primary/10 flex items-center justify-center">
    <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/></svg>
  </div>
  <div class="text-base font-semibold">{agent?.name ?? $t('agent.app')}</div>
  {#if agent?.role}
    <div class="text-xs text-base-content/70 max-w-xs text-center">{agent.role}</div>
  {:else}
    <div class="text-xs text-base-content/70">{$t('agent.appOwnWindow')}</div>
  {/if}
  <button
    class="btn btn-primary btn-sm gap-1.5"
    onclick={() => launchApp(agentId, agent?.name ?? 'App')}
  >
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
    {$t('agent.openApp')}
  </button>
</div>
