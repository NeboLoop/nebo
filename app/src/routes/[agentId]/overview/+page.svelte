<script lang="ts">
  import { getContext } from 'svelte';
  import { t } from 'svelte-i18n';
  import { launchApp } from '$lib/apps/launcher';
  import type { AgentPageContext } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const agentStatusVal = $derived(ctx.agentStatus(ctx.agentId));
</script>

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
