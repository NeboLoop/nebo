<script lang="ts">
  import { onMount } from 'svelte';
  import { ADVISOR_ROLE_COLORS } from '$lib/tokens.js';
  import type { Advisor } from '$lib/api/nebo';

  let advisors = $state<{ name: string; role: string; desc: string; priority: number; enabled: boolean }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAdvisors();
      if (resp?.advisors?.length) {
        advisors = resp.advisors.map((a: Advisor) => ({
          name: a.name,
          role: a.role || 'general',
          desc: a.description || '',
          priority: a.priority ?? 0,
          enabled: a.enabled ?? true,
        }));
      }
    } catch { /* keep mock data */ }
  });
</script>

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Advisors</h2>
  <p class="text-xs text-base-content/70">Manage advisor personas that provide different perspectives.</p>
</div>

<div class="flex flex-col gap-2 mb-6">
  {#each advisors as advisor}
    {@const roleColor = ADVISOR_ROLE_COLORS[advisor.role] || ADVISOR_ROLE_COLORS.general}
    <div class="flex items-start gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
      <div class="w-[3px] h-10 rounded-full shrink-0 mt-0.5 {roleColor.barClass}"></div>
      <div class="flex-1">
        <div class="text-sm font-semibold mb-0.5">{advisor.name}</div>
        <div class="text-sm font-mono uppercase tracking-wide mb-1 {roleColor.textClass}">{advisor.role}</div>
        <div class="text-sm leading-snug">{advisor.desc}</div>
      </div>
      <div class="flex items-center gap-2">
        <span class="px-1.5 py-0.5 rounded bg-base-200 text-sm font-mono">{advisor.priority}</span>
        <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={advisor.enabled} />
      </div>
    </div>
  {/each}
</div>

<button class="px-4 py-2 rounded-lg border border-dashed border-base-content/20 text-sm cursor-pointer hover:bg-base-200 hover:border-base-content/30 transition-colors w-full">+ Add Advisor</button>
