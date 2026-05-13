<script lang="ts">
  import { onMount } from 'svelte';

  let tasks = $state<{ label: string; primary: string; backup: string }[]>([]);
  let lanes = $state<{ name: string; description: string; concurrency: number; active: number; queued: number }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [modelsResp, lanesResp] = await Promise.all([
        api.listModels().catch(() => null),
        api.getLanes().catch(() => null),
      ]);
      // Task routing from models endpoint
      if (modelsResp?.taskRouting) {
        const routing = modelsResp.taskRouting as Record<string, Record<string, unknown>>;
        tasks = Object.entries(routing).map(([key, val]) => ({
          label: key.charAt(0).toUpperCase() + key.slice(1).replace(/_/g, ' '),
          primary: String(val?.primary || val?.model || ''),
          backup: String(val?.backup || val?.fallback || ''),
        }));
      }
      // Lane data from lanes endpoint
      if (lanesResp?.lanes?.length) {
        lanes = (lanesResp.lanes as Record<string, unknown>[]).map((l) => ({
          name: String(l.name),
          description: String(l.description || ''),
          concurrency: Number(l.concurrency || 1),
          active: Number(l.active || 0),
          queued: Number(l.queued || 0),
        }));
      }
    } catch { /* keep mock data */ }
  });
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Routing</h2>
  <p class="text-xs text-base-content/70">Configure which models handle which task types.</p>
</div>

<!-- Task routing -->
<div class="mb-8">
  <h3 class="text-base font-semibold mb-3">Task Routing</h3>
  <div class="rounded-lg border border-base-content/5 overflow-hidden">
    <table class="w-full">
      <thead>
        <tr class="bg-base-200/50">
          <th class="py-2 px-3.5 text-left text-sm font-semibold font-mono uppercase tracking-wide">Task</th>
          <th class="py-2 px-3.5 text-left text-sm font-semibold font-mono uppercase tracking-wide">Primary</th>
          <th class="py-2 px-3.5 text-left text-sm font-semibold font-mono uppercase tracking-wide">Backup</th>
        </tr>
      </thead>
      <tbody>
        {#each tasks as task}
          <tr class="border-t border-base-content/5">
            <td class="py-2.5 px-3.5 text-sm font-medium">{task.label}</td>
            <td class="py-2.5 px-3.5 text-sm font-mono">{task.primary}</td>
            <td class="py-2.5 px-3.5 text-sm font-mono">{task.backup}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>

<!-- Lane routing -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3">Lane Status</h3>
  <div class="flex flex-col gap-2">
    {#each lanes as lane}
      <div class="flex items-center justify-between p-3 rounded-lg border border-base-content/5 bg-base-100">
        <div>
          <span class="text-sm font-medium">{lane.name}</span>
          {#if lane.description}
            <span class="text-xs text-base-content/50 ml-2">{lane.description}</span>
          {/if}
        </div>
        <div class="flex items-center gap-3 text-xs text-base-content/60 font-mono">
          <span>{lane.active}/{lane.concurrency} active</span>
          {#if lane.queued > 0}
            <span class="text-warning">{lane.queued} queued</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
</div>
