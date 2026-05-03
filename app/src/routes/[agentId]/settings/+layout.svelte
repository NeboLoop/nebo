<script lang="ts">
  import { page } from '$app/stores';
  import { getContext } from 'svelte';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const agentStatusVal = $derived(ctx.agentStatus(ctx.agentId));

  const settingsSections = [
    { id: 'general', label: 'General' },
    { id: 'identity', label: 'Identity' },
    { id: 'persona', label: 'Persona' },
    { id: 'configure', label: 'Configure' },
    { id: 'workflows', label: 'Workflows' },
    { id: 'skills', label: 'Skills' },
    { id: 'memory', label: 'Memory' },
    { id: 'permissions', label: 'Permissions' },
  ];

  const activeSection = $derived($page.params.section || 'general');
</script>

<!-- Column 2: Settings nav -->
<div class="w-[260px] min-w-[260px] border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatusVal} />

  <div class="flex-1 overflow-y-auto">
    <div class="p-1.5 flex flex-col gap-0.5">
      {#each settingsSections as sec}
        <a
          href="/{agentId}/settings/{sec.id}"
          class="flex items-center w-full text-left py-1.5 px-2.5 rounded-md text-sm cursor-pointer transition-colors no-underline text-base-content {activeSection === sec.id ? 'bg-base-100 border border-base-300 shadow-sm font-medium' : 'bg-transparent border border-transparent hover:bg-base-200'}"
        >{sec.label}</a>
      {/each}
    </div>
  </div>
</div>

<!-- Column 3: Settings detail from child page -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  {@render children()}
</div>
