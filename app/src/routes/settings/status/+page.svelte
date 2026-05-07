<script lang="ts">
  import { onMount } from 'svelte';
  import Activity from 'lucide-svelte/icons/activity';
  import CheckCircle from 'lucide-svelte/icons/check-circle';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';

  let services = $state<{ name: string; status: 'operational' | 'degraded' | 'down'; latency: string }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [statusResp, lanesResp] = await Promise.all([
        api.getSimpleAgentStatus().catch(() => null),
        api.getLanes().catch(() => null),
      ]);

      const svcList: typeof services = [];

      // Agent runtime from status
      svcList.push({
        name: 'Agent Runtime',
        status: statusResp?.status === 'running' ? 'operational' : statusResp ? 'degraded' : 'down',
        latency: statusResp ? `${statusResp.tools} tools` : '—',
      });

      // Lanes as services
      if (lanesResp?.lanes?.length) {
        for (const l of lanesResp.lanes) {
          const lane = l as { name: string; active: number; queued: number; concurrency: number };
          svcList.push({
            name: `Lane: ${lane.name}`,
            status: lane.active > 0 || lane.queued === 0 ? 'operational' : 'degraded',
            latency: `${lane.active}/${lane.concurrency}`,
          });
        }
      }

      // NeboLoop status
      const neboStatus = await api.neboLoopAccountStatus().catch(() => null) as { connected?: boolean } | null;
      svcList.push({
        name: 'NeboLoop',
        status: neboStatus?.connected ? 'operational' : 'degraded',
        latency: '—',
      });

      if (svcList.length > 0) services = svcList;
    } catch { /* keep empty */ }
  });

  const operationalCount = $derived(services.filter(s => s.status === 'operational').length);
  const allOperational = $derived(operationalCount === services.length && services.length > 0);
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Status</h2>
  <p class="text-xs text-base-content/70">System health and service status.</p>
</div>

<!-- Overall status -->
<div class="p-4 rounded-xl border mb-6 {allOperational ? 'border-success/30 bg-success/5' : 'border-warning/30 bg-warning/5'}">
  <div class="flex items-center gap-3">
    {#if allOperational}
      <CheckCircle class="w-5 h-5 text-success" />
      <div>
        <div class="text-sm font-semibold text-success">All Systems Operational</div>
        <div class="text-xs text-base-content/50">{services.length} services monitored</div>
      </div>
    {:else}
      <AlertTriangle class="w-5 h-5 text-warning" />
      <div>
        <div class="text-sm font-semibold text-warning">Partial Degradation</div>
        <div class="text-xs text-base-content/50">{operationalCount} of {services.length} services operational</div>
      </div>
    {/if}
  </div>
</div>

<!-- Services -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3">Services</h3>
  <div class="flex flex-col gap-1.5">
    {#each services as service}
      <div class="flex items-center justify-between p-3 rounded-lg border border-base-content/5 bg-base-100">
        <div class="flex items-center gap-2.5">
          <div class="w-2 h-2 rounded-full {service.status === 'operational' ? 'bg-success' : 'bg-warning'}"></div>
          <span class="text-sm font-medium">{service.name}</span>
        </div>
        <div class="flex items-center gap-4">
          <span class="text-xs text-base-content/50 font-mono">{service.latency}</span>
          <span class="px-2 py-0.5 rounded text-sm font-semibold {service.status === 'operational' ? 'bg-success/10 text-success' : 'bg-warning/10 text-warning'}">
            {service.status === 'operational' ? 'Operational' : 'Degraded'}
          </span>
        </div>
      </div>
    {/each}
  </div>
</div>

<!-- System Info -->
<div class="p-4 rounded-lg bg-base-200/50 text-sm">
  <div class="flex items-center gap-2 mb-2">
    <Activity class="w-4 h-4 text-base-content/50" />
    <span class="font-semibold">System</span>
  </div>
  <p class="text-xs text-base-content/50">{services.length} services monitored</p>
</div>
