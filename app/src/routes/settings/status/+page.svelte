<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import Activity from 'lucide-svelte/icons/activity';
  import CheckCircle from 'lucide-svelte/icons/check-circle';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';

  let services = $state<{ name: string; status: 'operational' | 'degraded' | 'down'; latency: string; reason?: string }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [statusResp, lanesResp] = await Promise.all([
        api.getStatus().catch(() => null),
        api.getLanes().catch(() => null),
      ]);

      const svcList: typeof services = [];

      // Agent runtime from status. The backend emits "ready" when healthy
      // (providers present + tools registered); anything else carries a reason.
      const runtime = statusResp as { status?: string; reason?: string; tools?: number } | null;
      svcList.push({
        name: $t('settingsStatus.agentRuntime'),
        status: !runtime ? 'down' : runtime.status === 'ready' ? 'operational' : 'degraded',
        latency: runtime ? $t('settingsStatus.toolsCount', { values: { count: runtime.tools } }) : '—',
        reason: runtime?.reason || undefined,
      });

      // Lanes as services
      if (lanesResp?.lanes?.length) {
        for (const l of lanesResp.lanes) {
          const lane = l as { name: string; active: number; queued: number; concurrency: number };
          svcList.push({
            name: $t('settingsStatus.laneName', { values: { name: lane.name } }),
            status: lane.active > 0 || lane.queued === 0 ? 'operational' : 'degraded',
            latency: `${lane.active}/${lane.concurrency}`,
          });
        }
      }

      // NeboAI status
      const neboStatus = await api.neboAIAccountStatus().catch(() => null) as { connected?: boolean } | null;
      svcList.push({
        name: 'NeboAI',
        status: neboStatus?.connected ? 'operational' : 'degraded',
        latency: '—',
      });

      if (svcList.length > 0) services = svcList;
    } catch { /* keep empty */ }
  });

  const operationalCount = $derived(services.filter(s => s.status === 'operational').length);
  const allOperational = $derived(operationalCount === services.length && services.length > 0);
</script>

<SettingsHeader title={$t('settingsStatus.pageTitle')} description={$t('settingsStatus.pageDescription')} />

<!-- Overall status -->
<div class="p-4 rounded-xl border mb-6 {allOperational ? 'border-success/30 bg-success/5' : 'border-warning/30 bg-warning/5'}">
  <div class="flex items-center gap-3">
    {#if allOperational}
      <CheckCircle class="w-5 h-5 text-success" />
      <div>
        <div class="text-sm font-semibold text-success">{$t('settingsStatus.allOperational')}</div>
        <div class="text-xs text-base-content/50">{$t('settingsStatus.servicesMonitored', { values: { count: services.length } })}</div>
      </div>
    {:else}
      <AlertTriangle class="w-5 h-5 text-warning" />
      <div>
        <div class="text-sm font-semibold text-warning">{$t('settingsStatus.partialDegradation')}</div>
        <div class="text-xs text-base-content/50">{$t('settingsStatus.servicesOperational', { values: { operational: operationalCount, total: services.length } })}</div>
      </div>
    {/if}
  </div>
</div>

<!-- Services -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3">{$t('settingsStatus.services')}</h3>
  <div class="flex flex-col gap-1.5">
    {#each services as service}
      <div class="flex items-center justify-between p-3 rounded-lg border border-base-content/5 bg-base-100 gap-4">
        <div class="flex items-start gap-2.5 min-w-0">
          <div class="w-2 h-2 rounded-full mt-1.5 shrink-0 {service.status === 'operational' ? 'bg-success' : service.status === 'down' ? 'bg-error' : 'bg-warning'}"></div>
          <div class="min-w-0">
            <span class="text-sm font-medium">{service.name}</span>
            {#if service.reason}
              <p class="text-xs text-base-content/60 mt-0.5">{service.reason}</p>
            {/if}
          </div>
        </div>
        <div class="flex items-center gap-4 shrink-0">
          <span class="text-xs text-base-content/50 font-mono">{service.latency}</span>
          <span class="px-2 py-0.5 rounded text-sm font-semibold {service.status === 'operational' ? 'bg-success/10 text-success' : service.status === 'down' ? 'bg-error/10 text-error' : 'bg-warning/10 text-warning'}">
            {service.status === 'operational' ? $t('settingsStatus.operational') : service.status === 'down' ? $t('settingsStatus.down') : $t('settingsStatus.degraded')}
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
    <span class="font-semibold">{$t('settingsStatus.system')}</span>
  </div>
  <p class="text-xs text-base-content/50">{$t('settingsStatus.servicesMonitored', { values: { count: services.length } })}</p>
</div>
