<script lang="ts">
  import { onMount } from 'svelte';

  let skills = $state<{ name: string; bundled: boolean; enabled: boolean; tools: string[] }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listExtensions();
      const skillsList = (resp as unknown as Record<string, Record<string, unknown>[]>).skills;
      if (skillsList?.length) {
        skills = skillsList.map((ext) => ({
          name: String(ext.name),
          bundled: !!(ext.bundled ?? ext.isBundled ?? false),
          enabled: !!(ext.enabled ?? ext.isEnabled ?? true),
          tools: (ext.tools || ext.capabilities || []) as string[],
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

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Skills</h2>
  <p class="text-xs text-base-content/70">Manage installed skills and their capabilities.</p>
</div>

<div class="flex flex-col gap-1.5 mb-6">
  {#each skills as skill}
    <div class="flex items-start gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="flex-1">
        <div class="flex items-center gap-2 mb-1">
          <span class="text-sm font-semibold">{skill.name}</span>
          {#if skill.bundled}
            <span class="px-1.5 py-0.5 rounded text-sm font-mono bg-base-200">bundled</span>
          {:else}
            <span class="px-1.5 py-0.5 rounded text-sm font-mono bg-primary/10 text-primary">marketplace</span>
          {/if}
        </div>
        <div class="flex flex-wrap gap-1">
          {#each skill.tools as tool}
            <span class="px-1.5 py-0.5 rounded bg-base-200 text-sm font-mono">{tool}</span>
          {/each}
        </div>
      </div>
      <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={skill.enabled} onchange={() => toggleSkill(skill)} />
    </div>
  {/each}
</div>

<a href="/marketplace/skills" class="px-4 py-2 rounded-lg border border-dashed border-base-content/20 text-sm cursor-pointer hover:bg-base-200 transition-colors inline-block">Browse more skills &rarr;</a>
