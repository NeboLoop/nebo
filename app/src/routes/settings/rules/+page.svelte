<script lang="ts">
  import { onMount } from 'svelte';

  let rules = $state<{ section: string; rules: { enabled: boolean; text: string }[] }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.getAgentProfile() as { profile?: Record<string, unknown> } | null;
      // Agent rules come from the agent profile as a text block
      const profile = resp?.profile;
      if (profile?.agentRules) {
        const ruleLines = String(profile.agentRules).split('\n').filter((l: string) => l.trim());
        rules = [{
          section: 'Custom Rules',
          rules: ruleLines.map((line: string) => ({
            enabled: true,
            text: line.replace(/^[-*]\s*/, ''),
          })),
        }];
      }
    } catch { /* keep mock data */ }
  });
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Rules</h2>
  <p class="text-xs text-base-content/70">Define behavior constraints and guidelines for your agent.</p>
</div>

{#each rules as section}
  <div class="mb-5">
    <h3 class="text-sm font-semibold mb-2">{section.section}</h3>
    <div class="flex flex-col gap-1">
      {#each section.rules as rule}
        <div class="flex items-center gap-2.5 py-2 px-3 rounded-lg hover:bg-base-content/3 transition-colors">
          <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={rule.enabled} />
          <span class="flex-1 text-sm">{rule.text}</span>
        </div>
      {/each}
    </div>
  </div>
{/each}

<div class="flex gap-2 mt-4">
  <button class="px-4 py-2 rounded-lg border border-dashed border-base-content/20 text-sm cursor-pointer hover:bg-base-200 transition-colors">+ Add Section</button>
  <button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm cursor-pointer hover:bg-base-200 transition-colors ml-auto">Reset to Defaults</button>
</div>
