<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import Sidebar from '$lib/components/Sidebar.svelte';

  let automations = $state<{ id: string; name: string; trigger: string; schedule?: string; event?: string; agent: string; enabled: boolean }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listTasks();
      if (resp?.tasks?.length) {
        automations = (resp.tasks as unknown as Record<string, unknown>[]).map((t) => ({
          id: (t.id || t.name) as string,
          name: (t.name || t.label) as string,
          trigger: (t.trigger || 'schedule') as string,
          schedule: (t.schedule || '') as string,
          event: (t.event || '') as string,
          agent: (t.agentName || t.agent_name || '') as string,
          enabled: (t.enabled ?? t.is_enabled ?? true) as boolean,
        }));
      }
    } catch { /* keep mock data */ }
  });

  async function toggleAutomation(auto: typeof automations[0]) {
    auto.enabled = !auto.enabled;
    try {
      const api = await import('$lib/api/nebo');
      await api.toggleTask(auto.id || auto.name);
    } catch { /* local state already updated */ }
  }
</script>

<svelte:head><title>{$t('automate.pageTitle')}</title></svelte:head>

<div class="flex h-screen bg-base-100 text-base-content text-sm">
  <Sidebar activePage="chat" />
  <div class="flex-1 flex flex-col min-w-0 min-h-0">
    <div class="h-12 px-5 border-b border-base-content/10 flex items-center gap-3.5 shrink-0">
      <span class="text-sm font-semibold">{$t('automations.title')}</span>
      <div class="ml-auto h-7 w-[200px] rounded-md border border-base-content/10 bg-base-100 flex items-center px-2.5 gap-2 text-sm">
        <span class="font-mono">⌘K</span><span>{$t('nav.searchOrRun')}</span>
      </div>
    </div>

    <div class="flex-1 overflow-auto p-6">
      <div class="max-w-[800px]">
        <h1 class="text-xl font-bold tracking-tight mb-4">{$t('automations.title')}</h1>

        <div class="flex flex-col gap-2">
          {#each automations as auto}
            <div class="flex items-center gap-3.5 py-3.5 px-4 rounded-lg border border-base-content/5 bg-base-100">
              <div class="w-8 h-8 rounded-lg bg-base-200 grid place-items-center text-base shrink-0">{auto.trigger === 'schedule' ? '↻' : '⚡'}</div>
              <div class="flex-1 flex flex-col gap-0.5">
                <div class="text-sm font-semibold">{auto.name}</div>
                <div class="text-sm">{auto.trigger === 'schedule' ? auto.schedule : auto.event}</div>
                <div class="text-sm">{auto.agent}</div>
              </div>
              <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={auto.enabled} onchange={() => toggleAutomation(auto)} />
            </div>
          {/each}
        </div>

        <p class="mt-4 text-sm">{$t('automate.browsePrefix')} <a href="/marketplace" class="text-primary hover:underline">{$t('automate.marketplaceLink')}</a> {$t('automate.browseSuffix')}</p>
      </div>
    </div>
  </div>
</div>
