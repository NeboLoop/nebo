<script lang="ts">
  import { page } from '$app/stores';

  let { agentId, agentName, agentInitial, status } = $props<{
    agentId: string;
    agentName: string;
    agentInitial: string;
    status: string;
  }>();

  function statusLabel(s: string) {
    if (s === 'online') return 'Online';
    if (s === 'running') return 'Running';
    if (s === 'paused') return 'Paused';
    return 'Idle';
  }

  const activeTab = $derived.by(() => {
    const p = $page.url.pathname;
    if (p.includes('/settings')) return 'settings';
    if (p.includes('/runs')) return 'runs';
    return 'threads';
  });
</script>

<div class="h-11 px-3.5 border-b border-base-content/10 flex items-center gap-2 shrink-0">
  <div class="w-6 h-6 rounded-field flex items-center justify-center font-mono text-sm font-semibold shrink-0 bg-primary text-primary-content">{agentInitial}</div>
  <span class="text-sm font-semibold">{agentName}</span>
  <span class="text-xs ml-auto flex items-center gap-1.5 {status === 'online' ? 'text-success' : status === 'running' ? 'text-warning' : 'text-base-content/50'}">
    <span class="w-1.5 h-1.5 rounded-full {status === 'online' ? 'bg-success' : status === 'running' ? 'bg-warning animate-pulse' : 'bg-base-content/30'}"></span>
    {statusLabel(status)}
  </span>
</div>

<div role="tablist" class="tabs tabs-border w-full shrink-0">
  <a href="/{agentId}/threads" role="tab" class="tab flex-1 {activeTab === 'threads' ? 'tab-active' : ''}">Threads</a>
  <a href="/{agentId}/runs" role="tab" class="tab flex-1 {activeTab === 'runs' ? 'tab-active' : ''}">Runs</a>
  <a href="/{agentId}/settings" role="tab" class="tab flex-1 {activeTab === 'settings' ? 'tab-active' : ''}">Settings</a>
</div>
