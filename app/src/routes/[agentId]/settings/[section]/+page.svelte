<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { getContext } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { getActivityType } from '$lib/utils/workflowTypes';
  import type { AgentPageContext, WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const agentColor = $derived(ctx.agentColor);
  const skills = $derived(ctx.skills);
  const config = $derived(ctx.config);
  const workflowEntries = $derived(ctx.workflowEntries);
  const workflowStats = $derived(ctx.workflowStats);
  const devMode = $derived(ctx.devMode);

  const section = $derived($page.params.section);

  function createNewWorkflow() {
    const existing = workflowEntries.map(([name]: [string, WorkflowConfig]) => name);
    let idx = 1;
    let name = 'New Workflow';
    while (existing.includes(name)) {
      idx++;
      name = `New Workflow ${idx}`;
    }
    const wf = {
      trigger: { type: 'manual' as const },
      description: '',
      isActive: true,
      activities: [],
    };
    ctx.openWorkflow(name, wf);
  }

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

  // Delete confirmation triggered by ?delete=1 query param or button click
  let showDeleteConfirm = $state(false);

  $effect(() => {
    if ($page.url.searchParams.get('delete') === '1') {
      showDeleteConfirm = true;
    }
  });

  function statusLabel(s: string) {
    if (s === 'online') return 'Online';
    if (s === 'running') return 'Running';
    if (s === 'paused') return 'Paused';
    return 'Idle';
  }

  function triggerSummary(wf: WorkflowConfig): string {
    if (wf.trigger?.type === 'schedule') return wf.schedule || 'Scheduled';
    if (wf.trigger?.type === 'event') return `On ${wf.trigger.event || 'event'}`;
    return 'Manual trigger';
  }
</script>

<div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
  <span class="text-sm font-semibold">{agent?.name} &mdash; {settingsSections.find(s => s.id === section)?.label ?? 'Settings'}</span>
</div>
<div class="flex-1 overflow-y-auto p-6">
  <div class="max-w-[480px] flex flex-col gap-5">

    {#if section === 'general'}
      {@const gc = agent ? AGENT_COLORS_MAP[agent.color] : null}
      <div class="flex items-start gap-4 pb-5 border-b border-base-300">
        <div class="w-12 h-12 rounded-field flex items-center justify-center font-mono text-base font-semibold shrink-0 {gc?.bgClass} {gc?.inkClass}">{agent?.initial}</div>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <div class="text-sm font-semibold">{agent?.name}</div>
            {#if !agent?.editable}
              <span class="py-0.5 px-2 rounded bg-base-200 font-mono text-xs text-base-content/70">Read-only</span>
            {/if}
          </div>
          <div class="text-xs text-base-content/70">{agent?.role}</div>
          <div class="flex items-center gap-2 mt-1.5">
            <div class="w-[7px] h-[7px] rounded-full shrink-0 {ctx.agentStatus(agentId) === 'online' ? 'bg-success' : ctx.agentStatus(agentId) === 'running' ? 'bg-warning animate-pulse' : 'bg-base-content/30'}"></div>
            <span class="text-xs text-base-content/50">{statusLabel(ctx.agentStatus(agentId))}</span>
            {#if agentId !== 'assistant'}
              <button
                class="ml-1 py-0.5 px-2 rounded text-xs font-medium cursor-pointer border border-base-300 bg-base-100 hover:bg-base-200 transition-colors"
                onclick={() => ctx.toggleAgentStatus(agentId)}
              >{ctx.agentStatus(agentId) === 'paused' ? 'Activate' : 'Pause'}</button>
            {/if}
          </div>
        </div>
      </div>

      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5">
          <div class="text-xs text-base-content/70">This agent's configuration is managed by its <span class="font-mono">agent.json</span> and cannot be edited directly. Duplicate it to create an editable copy.</div>
        </div>
      {/if}

      {#if devMode}
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Model</div>
          <div class="text-sm font-mono">{config.model}</div>
        </div>
      {/if}

      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Skills</div>
        <div class="text-sm">{skills.length > 0 ? skills.join(', ') : 'None assigned'}</div>
      </div>

      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Workflows</div>
        <div class="text-sm">{workflowEntries.length} configured</div>
      </div>

      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Created</div>
        <div class="text-sm">Mar 12, 2026</div>
      </div>

      <!-- Danger zone -->
      {#if agent?.editable}
        <div class="border-t border-base-300 pt-5 mt-3">
          <div class="text-xs font-semibold uppercase tracking-wider text-error mb-2">Danger Zone</div>
          {#if showDeleteConfirm}
            <div class="rounded-lg border border-error/30 bg-error/5 p-4">
              <div class="text-sm font-medium mb-1">Delete {agent?.name}?</div>
              <div class="text-xs text-base-content/70 mb-3">This will permanently remove the agent, all threads, runs, and memory. This action cannot be undone.</div>
              <div class="flex items-center gap-2">
                <button class="btn btn-error btn-sm" onclick={() => showDeleteConfirm = false}>Delete Agent</button>
                <button class="btn btn-ghost btn-sm" onclick={() => showDeleteConfirm = false}>Cancel</button>
              </div>
            </div>
          {:else}
            <button class="btn btn-error btn-sm btn-outline" onclick={() => showDeleteConfirm = true}>Delete Agent</button>
          {/if}
        </div>
      {/if}

    {:else if section === 'identity'}
      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5 text-xs text-base-content/70">Managed by <span class="font-mono">agent.json</span> — read-only.</div>
      {/if}
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">Agent Name</span>
        <input type="text" value={agent?.name ?? ''} disabled={!agent?.editable} class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body disabled:opacity-60 disabled:cursor-not-allowed" />
      </label>
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">Role</span>
        <input type="text" value={agent?.role ?? ''} disabled={!agent?.editable} class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body disabled:opacity-60 disabled:cursor-not-allowed" />
      </label>
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider mb-1.5">Color</div>
        <div class="flex gap-2">
          {#each ['violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach'] as color}
            {@const c = AGENT_COLORS_MAP[color]}
            <button
              class="w-7 h-7 rounded-md border-2 transition-colors {c.bgClass} {agent?.color === color ? 'border-base-content' : 'border-transparent'} {agent?.editable ? 'cursor-pointer' : 'opacity-60 cursor-not-allowed'}"
              title={color}
              disabled={!agent?.editable}
            ></button>
          {/each}
        </div>
      </div>
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider mb-1.5">Status</div>
        <div class="flex items-center gap-1.5 text-sm">
          <div class="w-[7px] h-[7px] rounded-full shrink-0 {(agent?.status ?? 'idle') === 'online' ? 'bg-success' : (agent?.status ?? 'idle') === 'running' ? 'bg-warning animate-pulse' : 'bg-base-content/30'}"></div>
          {statusLabel(agent?.status ?? 'idle')}
        </div>
      </div>

    {:else if section === 'persona'}
      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5 text-xs text-base-content/70">Managed by <span class="font-mono">agent.json</span> — read-only.</div>
      {/if}
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">System Prompt</span>
        <div class="text-xs text-base-content/70 mb-1.5">From AGENT.md &mdash; defines personality, communication style, and judgment rules.</div>
        <textarea rows="8" placeholder="Describe this agent's personality, communication style, and approach..." disabled={!agent?.editable}
          class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-body leading-relaxed disabled:opacity-60 disabled:cursor-not-allowed">{config.persona}</textarea>
      </label>
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">Temperature</span>
        <div class="flex items-center gap-3">
          <input type="range" min="0" max="1" step="0.1" value="0.7" class="flex-1" disabled={!agent?.editable} />
          <span class="font-mono text-xs w-8 text-right">0.7</span>
        </div>
      </label>

    {:else if section === 'configure'}
      <div class="text-sm mb-1">Configuration from <span class="font-mono">agent.json</span>. These inputs customize how {agent?.name} operates.</div>

      {#if config.inputs.length === 0}
        <div class="text-center py-6 text-sm">No configurable inputs for this agent.</div>
      {:else}
        {#each config.inputs as _input}
          {@const input = _input as { key?: string; label?: string; required?: boolean; description?: string; type?: string; placeholder?: string; default?: string; options?: { value: string; label: string }[] }}
          <label class="block">
            <span class="block text-xs font-semibold uppercase tracking-wider mb-1">
              {input.label ?? input.key}
              {#if input.required}<span class="text-error">*</span>{/if}
            </span>
            {#if input.description}
              <span class="block text-sm mb-1.5">{input.description}</span>
            {/if}

            {#if input.type === 'textarea'}
              <textarea rows="3" placeholder={input.placeholder ?? ''}
                class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-body leading-relaxed">{input.default ?? ''}</textarea>
            {:else if input.type === 'select'}
              <select class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body">
                {#each input.options ?? [] as opt}
                  <option value={opt.value} selected={opt.value === input.default}>{opt.label}</option>
                {/each}
              </select>
            {:else}
              <input type="text" placeholder={input.placeholder ?? ''} value={input.default ?? ''}
                class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body" />
            {/if}
          </label>
        {/each}
      {/if}

    {:else if section === 'workflows'}
      <!-- Stats cards -->
      {#if workflowStats.totalRuns > 0}
        <div class="grid grid-cols-4 gap-2 mb-4">
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold">{workflowStats.totalRuns}</div>
            <div class="text-xs text-base-content/50">Total runs</div>
          </div>
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold text-success">{workflowStats.completed}</div>
            <div class="text-xs text-base-content/50">Completed</div>
          </div>
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold {workflowStats.failed > 0 ? 'text-error' : ''}">{workflowStats.failed}</div>
            <div class="text-xs text-base-content/50">Failed</div>
          </div>
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold font-mono">{workflowStats.avgDuration}</div>
            <div class="text-xs text-base-content/50">Avg duration</div>
          </div>
        </div>
      {/if}

      <!-- Header with canvas button -->
      <div class="flex items-center justify-between mb-3">
        <div class="text-sm">Automated sequences for {agent?.name}.</div>
        {#if workflowEntries.length > 0}
          <button
            class="flex items-center gap-1.5 py-1 px-2.5 rounded-lg border border-base-300 text-xs font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors"
            onclick={() => ctx.openCanvas()}
            title="Open canvas editor"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="8" y="14" width="7" height="7" rx="1"/><line x1="6.5" y1="10" x2="11.5" y2="14"/><line x1="17.5" y1="10" x2="11.5" y2="14"/></svg>
            Canvas
          </button>
        {/if}
      </div>

      {#if workflowEntries.length === 0}
        <div class="text-center py-8 text-sm">
          No workflows configured. Create one or install from the Marketplace.
        </div>
      {:else}
        <div class="flex flex-col gap-2">
          {#each workflowEntries as [name, wf]}
            {@const purchased = wf.source === 'marketplace'}
            <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
              <div class="flex items-start gap-3 p-3.5">
                <div class="w-[22px] h-[22px] rounded flex items-center justify-center text-sm shrink-0 mt-0.5 {wf.isActive !== false ? 'bg-primary/10 text-primary' : 'bg-base-200 text-base-content/40'}">
                  {#if wf.trigger?.type === 'schedule'}&#8635;{:else if wf.trigger?.type === 'event'}&#9889;{:else}&#9654;{/if}
                </div>

                <button class="flex-1 min-w-0 text-left cursor-pointer bg-transparent border-none p-0" onclick={() => ctx.openWorkflow(name, wf)}>
                  <div class="flex items-center gap-1.5">
                    <span class="text-sm font-medium">{name}</span>
                    {#if purchased}
                      <span class="py-0 px-1.5 rounded bg-base-200 text-xs font-mono">Marketplace</span>
                    {/if}
                    {#if wf.isActive === false}
                      <span class="py-0 px-1.5 rounded bg-base-200 text-xs text-base-content/50">Paused</span>
                    {/if}
                  </div>
                  <div class="text-xs text-base-content/70 mt-0.5 truncate">{wf.description}</div>
                  <div class="flex items-center gap-2 mt-1.5 flex-wrap">
                    <span class="text-xs text-base-content/50 font-mono">{triggerSummary(wf)}</span>
                    <span class="text-xs text-base-content/30">&middot;</span>
                    <span class="text-xs text-base-content/50 font-mono inline-flex items-center gap-1">{wf.activities?.length ?? 0} activities{#each [...new Set((wf.activities ?? []).map((a: WorkflowActivity) => a.type).filter(Boolean))] as t}<span class="inline-block" title={getActivityType(t).label}>{getActivityType(t).icon}</span>{/each}</span>
                    {#if wf.lastFired}
                      <span class="text-xs text-base-content/30">&middot;</span>
                      <span class="text-xs text-base-content/50 font-mono">Last: {wf.lastFired}</span>
                    {/if}
                    {#if wf.emit}
                      <span class="text-xs text-base-content/30">&middot;</span>
                      <span class="text-xs text-accent/70 font-mono">&#8594; {wf.emit}</span>
                    {/if}
                  </div>
                </button>

                <input type="checkbox" class="toggle toggle-sm toggle-primary shrink-0 mt-1" checked={wf.isActive !== false} role="switch" />
              </div>
            </div>
          {/each}
        </div>
      {/if}

      <button class="mt-3 w-full py-2.5 rounded-lg border border-dashed border-base-300 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={createNewWorkflow}>+ New workflow</button>

    {:else if section === 'skills'}
      <div class="text-xs text-base-content/70 mb-2">Skills assigned to {agent?.name}. Add more from the Marketplace.</div>
      {#each skills as skill}
        <div class="flex items-center gap-2.5 py-2 px-3 rounded-lg border border-base-300 bg-base-100">
          <div class="w-7 h-7 rounded-md bg-base-200 flex items-center justify-center text-sm shrink-0">&#9889;</div>
          <span class="text-sm font-medium flex-1">{skill}</span>
          <button class="text-sm text-error cursor-pointer bg-transparent border-none hover:opacity-70">Remove</button>
        </div>
      {/each}
      <a href="/marketplace/skills" class="inline-flex items-center gap-1 text-sm text-primary font-medium mt-1">+ Add from Marketplace &#8594;</a>

    {:else if section === 'memory'}
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">Permanent Memory</span>
        <textarea rows="4" placeholder="Standing instructions and preferences..."
          class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-body leading-relaxed"></textarea>
        <span class="block text-xs text-base-content/70 mt-1">Persists across all threads. Thread memory is scoped to each conversation.</span>
      </label>
      <div class="border-t border-base-300 pt-4">
        <div class="text-xs font-semibold uppercase tracking-wider mb-1.5">Memory Banks</div>
        <div class="flex flex-col gap-1.5">
          <div class="flex items-center justify-between py-1.5 px-3 rounded-lg border border-base-300 bg-base-100 text-sm">
            <span>Preferences</span>
            <span class="font-mono">24 entries</span>
          </div>
          <div class="flex items-center justify-between py-1.5 px-3 rounded-lg border border-base-300 bg-base-100 text-sm">
            <span>Entities</span>
            <span class="font-mono">12 entries</span>
          </div>
          <div class="flex items-center justify-between py-1.5 px-3 rounded-lg border border-base-300 bg-base-100 text-sm">
            <span>Daily</span>
            <span class="font-mono">3 entries</span>
          </div>
        </div>
      </div>

    {:else if section === 'permissions'}
      <div class="text-xs text-base-content/70 mb-2">Control what {agent?.name} can access and execute.</div>
      {#each [
        { id: 'files', label: 'File Access', desc: 'Read and write files on disk', on: true },
        { id: 'shell', label: 'Shell', desc: 'Execute terminal commands', on: true },
        { id: 'web', label: 'Web', desc: 'Make HTTP requests', on: true },
        { id: 'contacts', label: 'Contacts', desc: 'Access address book', on: false },
        { id: 'desktop', label: 'Desktop', desc: 'Screen capture and control', on: false },
      ] as perm}
        <div class="flex items-center gap-3 py-2">
          <div class="flex-1">
            <div class="text-sm font-medium">{perm.label}</div>
            <div class="text-xs text-base-content/70">{perm.desc}</div>
          </div>
          <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={perm.on} role="switch" aria-checked={perm.on} />
        </div>
      {/each}

    {:else}
      <div class="text-center py-10 text-sm">Unknown settings section.</div>
    {/if}

  </div>
</div>
