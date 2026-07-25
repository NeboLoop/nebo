<!--
  LearningControls — the per-employee self-improvement mode (Settings →
  employee → Approvals, above the per-operation controls). Three states,
  same segmented visual language as ApprovalControls:

    Learn freely (auto)  — the review fork writes learned skills directly
    Ask me first (staged) — learnings become Inbox approval cards (default
                            for newly hired employees)
    Off                   — no self-improvement review runs

  Reads the resolved entity config; writes ride the ONE canonical pathway —
  the entity-config PUT `learningMode` patch.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import GraduationCap from 'lucide-svelte/icons/graduation-cap';
  import Hand from 'lucide-svelte/icons/hand';
  import Ban from 'lucide-svelte/icons/ban';
  import * as api from '$lib/api/nebo';

  type Mode = 'auto' | 'staged' | 'off';

  let { agentId }: { agentId: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let mode = $state<Mode>('off');

  const modes: { value: Mode; labelKey: string; hintKey: string; icon: typeof Hand; active: string }[] = [
    {
      value: 'auto',
      labelKey: 'agentLearning.auto',
      hintKey: 'agentLearning.autoHint',
      icon: GraduationCap,
      active: 'bg-success/15 border-success/40 text-success',
    },
    {
      value: 'staged',
      labelKey: 'agentLearning.staged',
      hintKey: 'agentLearning.stagedHint',
      icon: Hand,
      active: 'bg-warning/15 border-warning/40 text-warning',
    },
    {
      value: 'off',
      labelKey: 'agentLearning.off',
      hintKey: 'agentLearning.offHint',
      icon: Ban,
      active: 'bg-error/15 border-error/40 text-error',
    },
  ];

  const currentHint = $derived(modes.find((m) => m.value === mode)?.hintKey ?? '');

  async function load() {
    loading = true;
    try {
      const resp = (await api.getEntityConfig('agent', agentId)) as {
        config?: { learningMode?: string };
      };
      const v = (resp.config?.learningMode ?? 'off').toLowerCase();
      mode = v === 'auto' || v === 'staged' ? (v as Mode) : 'off';
    } catch {
      mode = 'off';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (agentId) load();
  });

  async function setMode(value: Mode) {
    if (saving || value === mode) return;
    const prev = mode;
    mode = value;
    saving = true;
    try {
      await api.updateEntityConfig('agent', agentId, { learningMode: value });
    } catch {
      mode = prev;
    } finally {
      saving = false;
    }
  }
</script>

<!-- Grouped-settings idiom (Apple-style): uppercase group caption, the
     control, then a muted footnote below explaining the CURRENT state. -->
<div class="max-w-2xl">
  {#if !loading}
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('agentLearning.title')}</div>
    <div class="join">
      {#each modes as m (m.value)}
        <button
          class="join-item flex items-center gap-1.5 px-3 py-1.5 text-xs border transition-colors cursor-pointer {mode === m.value
            ? m.active
            : 'bg-base-100 border-base-content/10 text-base-content/40 hover:text-base-content/70 hover:bg-base-200'}"
          aria-pressed={mode === m.value}
          disabled={saving}
          onclick={() => setMode(m.value)}
        >
          <m.icon class="w-3.5 h-3.5" />{$t(m.labelKey)}
        </button>
      {/each}
    </div>
    <p class="text-xs text-base-content/60 mt-1.5">{$t(currentHint)}</p>
  {/if}
</div>
