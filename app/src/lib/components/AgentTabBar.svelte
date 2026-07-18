<script lang="ts">
  import { page } from '$app/stores';
  import { t } from 'svelte-i18n';

  let { agentId, agentName, agentInitial, status, isApp = false } = $props<{
    agentId: string;
    agentName: string;
    agentInitial: string;
    status: string;
    isApp?: boolean;
  }>();

  function statusLabel(s: string) {
    if (s === 'online') return 'common.online';
    if (s === 'running') return 'components.agentTabBar.running';
    if (s === 'paused') return 'common.paused';
    return 'components.agentTabBar.idle';
  }

  const activeTab = $derived.by(() => {
    const p = $page.url.pathname;
    if (p.includes('/settings')) return 'settings';
    if (p.includes('/runs')) return 'runs';
    if (p.includes('/overview')) return 'overview';
    return 'threads';
  });
</script>

<div class="h-11 px-3.5 border-b border-base-content/10 flex items-center gap-2 shrink-0">
  <div class="w-6 h-6 rounded-field flex items-center justify-center font-mono text-sm font-semibold shrink-0 bg-primary text-primary-content">
    {#if isApp}
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/></svg>
    {:else}
      {agentInitial}
    {/if}
  </div>
  <span class="text-sm font-semibold">{agentName}</span>
  <span class="text-xs ml-auto flex items-center gap-1.5 {status === 'online' ? 'text-success' : status === 'running' ? 'text-warning' : 'text-base-content/50'}">
    <span class="w-1.5 h-1.5 rounded-full {status === 'online' ? 'bg-success' : status === 'running' ? 'bg-warning animate-pulse' : 'bg-base-content/30'}"></span>
    {$t(statusLabel(status))}
  </span>
</div>

<div role="tablist" class="tabs tabs-border w-full shrink-0">
  {#if isApp}
    <a href="/{agentId}/overview" role="tab" class="tab flex-1 {activeTab === 'overview' ? 'tab-active' : ''}">{$t('components.agentTabBar.overview')}</a>
  {:else}
    <a href="/{agentId}/threads" role="tab" class="tab flex-1 {activeTab === 'threads' ? 'tab-active' : ''}">{$t('components.agentTabBar.chats')}</a>
  {/if}
  <a href="/{agentId}/runs" role="tab" class="tab flex-1 {activeTab === 'runs' ? 'tab-active' : ''}">{$t('components.agentTabBar.runs')}</a>
  <a href="/{agentId}/settings" role="tab" class="tab flex-1 {activeTab === 'settings' ? 'tab-active' : ''}">{$t('nav.settings')}</a>
</div>
