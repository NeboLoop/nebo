<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import X from 'lucide-svelte/icons/x';
  import Maximize2 from 'lucide-svelte/icons/maximize-2';
  import Minimize2 from 'lucide-svelte/icons/minimize-2';

  interface Props {
    agentId: string;
    agentName: string;
    initialSection?: string;
    onclose: () => void;
  }

  let { agentId, agentName, initialSection = 'general', onclose }: Props = $props();

  let section = $state('general');
  $effect(() => { section = initialSection; });
  let agent = $state<Record<string, unknown> | null>(null);
  let loading = $state(true);
  let expanded = $state(false);

  const sections = [
    { id: 'general', label: 'General' },
    { id: 'runs', label: 'Runs' },
    { id: 'persona', label: 'Persona' },
    { id: 'workflows', label: 'Workflows' },
    { id: 'skills', label: 'Skills' },
    { id: 'memory', label: 'Memory' },
    { id: 'permissions', label: 'Permissions' },
  ];

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getAgent(agentId);
      agent = resp as unknown as Record<string, unknown>;
    } catch { /* keep null */ }
    loading = false;
  });

  function toggleExpanded() {
    expanded = !expanded;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose();
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="fixed inset-0 z-[80] flex items-center justify-center {expanded ? 'p-0' : 'p-6'}" role="dialog" aria-modal="true" aria-label="{agentName} settings">
  <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" role="presentation" onclick={onclose}></div>

  <div class="relative overflow-hidden flex flex-col bg-base-100 border border-base-content/10 shadow-2xl transition-all duration-200 {expanded ? 'w-full h-full rounded-none' : 'w-full max-w-2xl h-[70vh] rounded-2xl'}">
    <!-- Header -->
    <div class="flex items-center justify-between px-5 py-3.5 border-b border-base-content/10 shrink-0">
      <div class="flex items-center gap-2">
        <span class="text-sm font-semibold">{agentName}</span>
        <span class="text-xs text-base-content/50">Settings</span>
      </div>
      <div class="flex items-center gap-1">
        <button
          class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200/50 transition-colors cursor-pointer"
          onclick={toggleExpanded}
          title={expanded ? 'Compact view' : 'Full view'}
        >
          {#if expanded}
            <Minimize2 class="w-3.5 h-3.5 text-base-content/50" />
          {:else}
            <Maximize2 class="w-3.5 h-3.5 text-base-content/50" />
          {/if}
        </button>
        <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200/50 transition-colors cursor-pointer" onclick={onclose}>
          <X class="w-4 h-4" />
        </button>
      </div>
    </div>

    <!-- Body -->
    <div class="flex flex-1 min-h-0">
      <!-- Nav -->
      <div class="w-40 shrink-0 border-r border-base-content/10 py-2 px-1.5 overflow-y-auto">
        {#each sections as sec}
          <button
            class="w-full text-left py-1.5 px-2.5 rounded-md text-sm cursor-pointer transition-colors {section === sec.id ? 'bg-base-200 font-medium' : 'hover:bg-base-200/50'}"
            onclick={() => section = sec.id}
          >{sec.label}</button>
        {/each}
      </div>

      <!-- Content -->
      <div class="flex-1 overflow-y-auto p-5">
        {#if loading}
          <div class="flex items-center justify-center py-12 text-sm text-base-content/50">Loading...</div>
        {:else if !agent}
          <div class="flex items-center justify-center py-12 text-sm text-base-content/50">Failed to load agent</div>
        {:else}
          <div class="max-w-[400px] flex flex-col gap-4">

            {#if section === 'general'}
              <div>
                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Name</div>
                <div class="text-sm">{agent.name}</div>
              </div>
              <div>
                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Description</div>
                <div class="text-sm">{agent.description || 'No description'}</div>
              </div>
              <div>
                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Version</div>
                <div class="text-sm font-mono">{agent.version || '—'}</div>
              </div>
              <div>
                <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Status</div>
                <div class="flex items-center gap-1.5 text-sm">
                  <div class="w-[7px] h-[7px] rounded-full {agent.isEnabled ? 'bg-success' : 'bg-base-content/30'}"></div>
                  {agent.isEnabled ? 'Enabled' : 'Disabled'}
                </div>
              </div>

            {:else if section === 'runs'}
              <div class="text-sm text-base-content/70 mb-2">View automation runs for {agentName}.</div>
              <button
                class="py-2 px-3 rounded-lg border border-base-300 text-sm font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors"
                onclick={() => { onclose(); goto(`/${agentId}/runs`); }}
              >View Run History</button>

            {:else if section === 'persona'}
              <div class="text-xs text-base-content/70 mb-1">System prompt from AGENT.md</div>
              <textarea
                rows="10"
                readonly
                class="w-full py-2 px-2.5 rounded-md border border-base-300 text-sm bg-base-200/30 outline-none resize-y font-body leading-relaxed"
              >{agent.agentMd || agent.persona || 'No persona defined'}</textarea>

            {:else if section === 'workflows'}
              <div class="text-sm text-base-content/70 mb-2">Automated workflows for {agentName}.</div>
              <button
                class="py-2 px-3 rounded-lg border border-base-300 text-sm font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors"
                onclick={() => { onclose(); goto(`/${agentId}/settings/workflows`); }}
              >Manage Workflows</button>

            {:else if section === 'skills'}
              <div class="text-xs text-base-content/70 mb-2">Skills assigned to {agentName}.</div>
              {#if Array.isArray(agent.skills) && agent.skills.length > 0}
                {#each agent.skills as skill}
                  <div class="flex items-center gap-2.5 py-2 px-3 rounded-lg border border-base-300 bg-base-100">
                    <div class="w-6 h-6 rounded-md bg-base-200 flex items-center justify-center text-xs shrink-0">&#9889;</div>
                    <span class="text-sm font-medium">{skill}</span>
                  </div>
                {/each}
              {:else}
                <div class="text-sm text-base-content/50">No skills assigned.</div>
              {/if}
              <a href="/marketplace/skills" class="inline-flex items-center gap-1 text-sm text-primary font-medium mt-1" onclick={onclose}>+ Add from Marketplace</a>

            {:else if section === 'memory'}
              <div class="text-xs text-base-content/70 mb-2">Memory and context for {agentName}.</div>
              <button
                class="py-2 px-3 rounded-lg border border-base-300 text-sm font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors"
                onclick={() => { onclose(); goto(`/${agentId}/settings/memory`); }}
              >Manage Memory</button>

            {:else if section === 'permissions'}
              <div class="text-xs text-base-content/70 mb-2">Capabilities for {agentName}.</div>
              {#each [
                { label: 'Storage', desc: 'App-scoped key-value storage', on: true },
                { label: 'Agent Invoke', desc: 'Call other agents', on: true },
                { label: 'Network', desc: 'Outbound HTTP requests', on: true },
                { label: 'File Access', desc: 'Read and write files', on: false },
                { label: 'Shell', desc: 'Execute terminal commands', on: false },
              ] as perm}
                <div class="flex items-center gap-3 py-2">
                  <div class="flex-1">
                    <div class="text-sm font-medium">{perm.label}</div>
                    <div class="text-xs text-base-content/70">{perm.desc}</div>
                  </div>
                  <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={perm.on} role="switch" aria-checked={perm.on} />
                </div>
              {/each}
            {/if}

          </div>
        {/if}
      </div>
    </div>
  </div>
</div>
