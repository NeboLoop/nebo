<script lang="ts">
  import { onMount } from 'svelte';
  import type { ExtensionInfo } from '$lib/api/nebo';

  let skills = $state<ExtensionInfo[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listExtensions();
      skills = resp.extensions;
    } catch { /* leave list empty on error */ }
  });

  async function toggleSkill(skill: ExtensionInfo) {
    skill.enabled = !skill.enabled;
    try {
      const api = await import('$lib/api/nebo');
      await api.toggleSkill(skill.name);
    } catch { /* local state already updated */ }
  }
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Skills</h2>
  <p class="text-xs text-base-content/70">Manage installed skills and their capabilities.</p>
</div>

<div class="flex flex-col gap-1.5 mb-6">
  {#each skills as skill}
    <div class="flex items-start gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="flex-1">
        <div class="flex items-center gap-2 mb-1">
          <span class="text-sm font-medium">{skill.name}</span>
          <span class="px-1.5 py-0.5 rounded text-xs font-mono {skill.source === 'user' ? 'bg-primary/10 text-primary' : 'bg-base-200'}">{skill.source}</span>
        </div>
        <div class="flex flex-wrap gap-1">
          {#each skill.capabilities as capability}
            <span class="px-1.5 py-0.5 rounded bg-base-200 text-xs font-mono">{capability}</span>
          {/each}
        </div>
      </div>
      <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={skill.enabled} onchange={() => toggleSkill(skill)} />
    </div>
  {/each}
</div>

<a href="/marketplace/skills" class="px-4 py-2 rounded-lg border border-dashed border-base-content/20 text-sm cursor-pointer hover:bg-base-200 transition-colors inline-block">Browse more skills &rarr;</a>
