<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';

  let skills = $state<{ name: string; bundled: boolean; enabled: boolean; tools: string[] }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listTools();
      if (resp?.tools?.length) {
        skills = (resp.tools as Record<string, unknown>[]).map((t) => ({
          name: t.name as string,
          bundled: (t.bundled ?? t.is_bundled ?? false) as boolean,
          enabled: (t.enabled ?? t.is_enabled ?? true) as boolean,
          tools: (t.tools || t.capabilities || []) as string[],
        }));
      }
    } catch { /* keep mock data */ }
  });

  async function toggleSkill(skill: typeof skills[0]) {
    skill.enabled = !skill.enabled;
    try {
      const api = await import('$lib/api/nebo');
      await api.toggleSkill(skill.name);
    } catch { /* local state already updated */ }
  }
</script>

<svelte:head><title>Skills - Nebo</title></svelte:head>

<div class="flex h-screen bg-base-100 text-base-content text-sm">
  <Sidebar activePage="skills" />
  <div class="flex-1 flex flex-col min-w-0 min-h-0">
    <div class="h-12 px-5 border-b border-base-content/10 flex items-center gap-3.5 shrink-0">
      <span class="text-sm font-semibold">Skills</span>
      <div class="ml-auto h-7 w-[200px] rounded-md border border-base-content/10 bg-base-100 flex items-center px-2.5 gap-2 text-sm">
        <span class="font-mono">⌘K</span><span>Search or run…</span>
      </div>
    </div>

    <div class="flex-1 overflow-auto p-6">
      <div class="max-w-[800px]">
        <h1 class="text-xl font-bold tracking-tight mb-4">Installed Skills</h1>

        <div class="grid grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-2.5">
          {#each skills as skill}
            <div class="p-3.5 rounded-lg border border-base-content/5 bg-base-100 flex flex-col gap-2">
              <div class="flex items-center justify-between">
                <span class="text-sm font-semibold">{skill.name}</span>
                <div class="flex items-center gap-1.5">
                  {#if skill.bundled}
                    <span class="text-sm px-1.5 py-0.5 rounded bg-base-200">Bundled</span>
                  {/if}
                  <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={skill.enabled} onchange={() => toggleSkill(skill)} />
                </div>
              </div>
              <div class="flex flex-wrap gap-1">
                {#each skill.tools as tool}
                  <span class="px-1.5 py-0.5 rounded bg-base-200 font-mono text-xs">{tool}</span>
                {/each}
              </div>
            </div>
          {/each}
        </div>

        <div class="mt-7">
          <div class="flex items-baseline mb-3.5">
            <span class="text-base font-semibold">Skill Store</span>
            <a href="/marketplace/skills" class="ml-auto text-sm hover:text-base-content">Browse all →</a>
          </div>
          <p class="text-sm mb-3.5">Discover new capabilities from the marketplace.</p>
          <a href="/marketplace/skills" class="inline-block px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-medium hover:opacity-90 transition-opacity">Browse Marketplace</a>
        </div>
      </div>
    </div>
  </div>
</div>
