<script lang="ts">
  import { onMount } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import type { Agent } from '$lib/api/nebo';

  let agents = $state<{ id: string; name: string; role: string; initial: string; status: string; color: string }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        agents = resp.agents.map((a: Agent) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        }));
      }
    } catch { /* keep mock data */ }
  });
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Agents</h2>
  <p class="text-xs text-base-content/70">View and manage your installed agents.</p>
</div>

<div class="flex flex-col gap-1.5">
  {#each agents as agent}
    {@const colors = AGENT_COLORS_MAP[agent.color] || AGENT_COLORS_MAP.violet}
    <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="w-9 h-9 rounded-lg grid place-items-center text-sm font-semibold shrink-0 {colors.bgClass} {colors.inkClass}">{agent.initial}</div>
      <div class="flex-1">
        <div class="text-sm font-semibold">{agent.name}</div>
        <div class="text-xs text-base-content/50">{agent.role}</div>
      </div>
      <span class="px-2 py-0.5 rounded text-sm font-medium {agent.status === 'online' ? 'bg-success/10 text-success' : agent.status === 'running' ? 'bg-info/10 text-info' : 'bg-base-200 text-base-content/60'}">{agent.status}</span>
      <button class="px-3 py-1 rounded-md border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors">Configure</button>
    </div>
  {/each}
</div>
