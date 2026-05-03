<script lang="ts">
  import { onMount } from 'svelte';
  import ColorCalendarShell from '$lib/components/ColorCalendarShell.svelte';
  import MiniMonth from '$lib/components/MiniMonth.svelte';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import WorkflowBuilder from '$lib/components/workflow/WorkflowBuilder.svelte';
  import { AGENTS, AGENT_ID_MAP } from '$lib/data.js';
  import { AGENT_COLORS } from '$lib/tokens.js';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar.js';
  import { getScheduleAgents, runsPerWeek, userScheduleItems, loadScheduleFromAPI } from '$lib/stores/schedule.js';
  import * as api from '$lib/api/nebo';
  const sidebarCollapsed = sidebarCollapsedFor('schedule');

  onMount(() => { loadScheduleFromAPI(); });

  let view = $state('day');
  let selectedDate = $state(new Date());

  // ── Workflow Canvas Modal ─────────────────────────────────────────
  let canvasAgentFull = $state<string | null>(null);
  let canvasWorkflowsData = $state<Record<string, Record<string, unknown>>>({});
  const showCanvasModal = $derived(!!canvasAgentFull);

  const canvasWorkflows = $derived(canvasWorkflowsData);
  const canvasAgentName = $derived.by(() => {
    if (!canvasAgentFull) return '';
    const shortId = (AGENT_ID_MAP as Record<string, string>)[canvasAgentFull];
    const a = shortId ? AGENTS.find(x => x.id === shortId) : null;
    return a?.name ?? canvasAgentFull;
  });

  async function handleOpenCanvas(agentFull: string) {
    canvasAgentFull = agentFull;
    canvasWorkflowsData = {};
    try {
      const resp = await api.listAgentWorkflows(agentFull);
      const workflows = resp?.workflows as Record<string, unknown>[] | undefined;
      if (workflows?.length) {
        const wfs: Record<string, Record<string, unknown>> = {};
        for (const wf of workflows) {
          const name = String(wf.bindingName || wf.workflowRef || wf.id || '');
          let triggerConfig: Record<string, unknown> = {};
          try { triggerConfig = typeof wf.triggerConfig === 'string' ? JSON.parse(wf.triggerConfig) : (wf.triggerConfig as Record<string, unknown>) || {}; } catch {}
          wfs[name] = {
            trigger: { type: wf.triggerType, ...triggerConfig },
            description: wf.description || name,
            isActive: wf.isActive,
            emit: wf.emit,
            activities: (wf.activities as unknown[]) || [],
          };
        }
        canvasWorkflowsData = wfs;
      }
    } catch {}
  }

  function handleCanvasSave(workflows: Record<string, Record<string, unknown>>) {
    // TODO: persist to backend via API
    canvasWorkflowsData = workflows;
    canvasAgentFull = null;
  }

  const schedAgents = $derived(getScheduleAgents($userScheduleItems));
  let enabled = $state<Record<string, boolean>>({});

  // Initialize enabled state when agents list is first available
  $effect(() => {
    for (const id of schedAgents) {
      if (!(id in enabled)) enabled[id] = true;
    }
  });

  function toggleAgent(id: string) { enabled[id] = !enabled[id]; }

  // Sidebar resize
  const SIDEBAR_MIN = 180;
  const SIDEBAR_DEFAULT = 220;
  const SIDEBAR_MAX_PCT = 0.3;
  let sidebarWidth = $state(SIDEBAR_DEFAULT);
  let sidebarResizing = $state(false);
  let containerEl = $state<HTMLDivElement | null>(null);

  function startSidebarResize(e: MouseEvent) {
    e.preventDefault();
    sidebarResizing = true;
    const onMove = (ev: MouseEvent) => {
      if (!containerEl) return;
      const rect = containerEl.getBoundingClientRect();
      const newWidth = ev.clientX - rect.left;
      const maxWidth = rect.width * SIDEBAR_MAX_PCT;
      sidebarWidth = Math.max(SIDEBAR_MIN, Math.min(maxWidth, newWidth));
    };
    const onUp = () => {
      sidebarResizing = false;
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  }
</script>

<svelte:head><title>Schedule - Nebo</title></svelte:head>

<div class="flex-1 flex min-h-0 {sidebarResizing ? 'select-none' : ''}" bind:this={containerEl}>
<!-- Left panel: agent toggles -->
<div class="{$sidebarCollapsed ? 'w-12 min-w-12 border-r border-base-300' : ''} flex flex-col bg-base-200 shrink-0 transition-all duration-150" style={$sidebarCollapsed ? '' : `width:${sidebarWidth}px; min-width:${SIDEBAR_MIN}px`}>
  <div class="h-11 border-b border-base-300 flex items-center shrink-0 {$sidebarCollapsed ? 'justify-center' : 'px-3.5 justify-between'}">
    {#if !$sidebarCollapsed}
      <span class="text-sm font-semibold flex-1">Agents</span>
    {/if}
    <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none shrink-0" onclick={() => $sidebarCollapsed = !$sidebarCollapsed} title={$sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.2"/><line x1="5.5" y1="3" x2="5.5" y2="13" stroke="currentColor" stroke-width="1.2"/></svg>
    </button>
  </div>

  <div class="flex-1 overflow-y-auto p-1.5">
    {#if $sidebarCollapsed}
      <div class="flex flex-col items-center gap-1 py-1">
        {#each schedAgents as id}
          {@const a = AGENTS.find(x => x.id === id)}
          {@const c = (AGENT_COLORS as Record<string, typeof AGENT_COLORS[keyof typeof AGENT_COLORS]>)[id]}
          {@const on = enabled[id] ?? true}
          {#if a && c}
            <button
              class="w-8 h-8 rounded-md flex items-center justify-center font-mono text-sm font-semibold shrink-0 cursor-pointer border-none transition-opacity {c.fillClass} {c.textClass} {on ? 'opacity-100' : 'opacity-40'}"
              onclick={() => toggleAgent(id)}
              title={a.name}
            >{a.name.charAt(0)}</button>
          {/if}
        {/each}
      </div>
    {:else}
      {#each schedAgents as id}
        {@const a = AGENTS.find(x => x.id === id)}
        {@const c = (AGENT_COLORS as Record<string, typeof AGENT_COLORS[keyof typeof AGENT_COLORS]>)[id]}
        {@const on = enabled[id] ?? true}
        {#if a && c}
          <label class="flex items-center gap-2.5 px-2.5 py-1.5 rounded-md cursor-pointer text-sm transition-opacity {on ? 'opacity-100' : 'opacity-50'}">
            <input type="checkbox" class="checkbox checkbox-sm {c.checkboxClass}" checked={on} onchange={() => toggleAgent(id)} />
            <span class="flex-1 font-medium">{a.name}</span>
            <span class="font-mono text-xs text-base-content/70">{runsPerWeek(id, $userScheduleItems)}/wk</span>
          </label>
        {/if}
      {/each}
    {/if}
  </div>

  {#if !$sidebarCollapsed}
    <MiniMonth {selectedDate} onselect={(d: Date) => selectedDate = d} />
  {/if}
  <UserMenu collapsed={$sidebarCollapsed} />
</div>

<!-- Resize handle -->
{#if !$sidebarCollapsed}
  <div
    class="w-0 shrink-0 cursor-col-resize relative z-10 group"
    onmousedown={startSidebarResize}
    role="separator"
    aria-orientation="vertical"
  >
    <div class="absolute inset-y-0 -left-2 -right-2"></div>
    <div class="absolute inset-y-0 -left-px w-0.5 bg-primary/30 opacity-0 group-hover:opacity-100 transition-opacity duration-300 delay-150 {sidebarResizing ? '!opacity-100' : ''}"></div>
    <div class="absolute top-1/2 -translate-y-1/2 -left-1.5 w-3 h-8 rounded-full bg-base-300 border border-base-content/10 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity duration-300 delay-150 {sidebarResizing ? '!opacity-100' : ''}">
      <div class="flex flex-col gap-0.5">
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
      </div>
    </div>
  </div>
{/if}

<!-- Main: Calendar -->
<ColorCalendarShell bind:view bind:selectedDate {enabled} onopencanvas={handleOpenCanvas} />
</div>

<!-- Workflow Canvas Builder — full-screen overlay -->
{#if showCanvasModal && canvasAgentFull}
  <div class="fixed inset-0 z-[60] flex flex-col" data-modal-open>
    <div class="absolute inset-0 bg-black/40" role="presentation"></div>
    <div class="relative flex flex-col flex-1 m-4 rounded-2xl bg-base-100 border border-base-300 shadow-2xl z-10 overflow-hidden">
      <div class="flex items-center justify-between px-5 py-3 border-b border-base-content/10 shrink-0">
        <div class="flex items-center gap-3">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="8" y="14" width="7" height="7" rx="1"/><line x1="6.5" y1="10" x2="11.5" y2="14"/><line x1="17.5" y1="10" x2="11.5" y2="14"/></svg>
          <div>
            <div class="text-sm font-semibold">{canvasAgentName} — Workflow Builder</div>
            <div class="text-xs text-base-content/50">{Object.keys(canvasWorkflows).length} workflows</div>
          </div>
        </div>
        <button class="w-8 h-8 rounded-lg flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={() => canvasAgentFull = null}>&times;</button>
      </div>
      <div class="flex-1 min-h-0">
        <WorkflowBuilder
          workflows={canvasWorkflows}
          agentId={canvasAgentFull}
          agentName={canvasAgentName}
          onclose={() => canvasAgentFull = null}
          onsave={handleCanvasSave}
        />
      </div>
    </div>
  </div>
{/if}
