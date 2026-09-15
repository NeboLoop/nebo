<!--
  RunLimitControls — the owner's per-run spending limit for one employee
  (Settings → employee → General, below Memory isolation).

  A package's token_budget is its author's cost estimate and is never
  enforced. This is the only ceiling a run has, and it is the owner's: off by
  default. When a run reaches it, the employee gets one last turn to report
  what it has, then the run is recorded as stopped at your limit — not failed,
  nothing lost.

  Reads the agent's frontmatter budget config; writes ride the ONE canonical
  pathway — the agent PUT `runSpendCapCents` field.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import * as api from '$lib/api/nebo';

  let { agentId }: { agentId: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let saved = $state(false);
  /** Dollars as typed; '' = no limit. */
  let dollars = $state('');
  let savedCents = $state(0);
  let saveTimer: ReturnType<typeof setTimeout> | null = null;

  async function load() {
    loading = true;
    try {
      const resp = (await api.getAgent(agentId)) as { agent?: { frontmatter?: string } };
      const fm = JSON.parse(resp.agent?.frontmatter || '{}');
      const cents = Number(fm?.budget?.run_spend_cap_cents ?? 0);
      savedCents = Number.isFinite(cents) && cents > 0 ? Math.round(cents) : 0;
      dollars = savedCents > 0 ? (savedCents / 100).toFixed(2) : '';
    } catch {
      savedCents = 0;
      dollars = '';
    } finally {
      loading = false;
    }
  }

  function centsFromInput(): number {
    const n = Number.parseFloat(dollars);
    return Number.isFinite(n) && n > 0 ? Math.round(n * 100) : 0;
  }

  // Saves as you type, the way every console form does — no Save button.
  function onInput() {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(save, 700);
  }

  async function save() {
    const cents = centsFromInput();
    if (cents === savedCents) return;
    saving = true;
    saved = false;
    try {
      await api.updateAgent(agentId, { runSpendCapCents: cents });
      savedCents = cents;
      saved = true;
      setTimeout(() => (saved = false), 1500);
    } finally {
      saving = false;
    }
  }

  onMount(load);
</script>

<div class="max-w-2xl">
  <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('runLimit.title')}</div>
  <label class="input input-sm input-bordered flex items-center gap-2 w-48">
    <span class="text-base-content/50">$</span>
    <input
      type="number"
      inputmode="decimal"
      min="0"
      step="0.50"
      class="grow"
      placeholder={$t('runLimit.offPlaceholder')}
      bind:value={dollars}
      oninput={onInput}
      onblur={save}
      disabled={loading}
    />
    {#if saving}
      <span class="loading loading-spinner loading-xs"></span>
    {:else if saved}
      <span class="text-xs text-success">{$t('common.saved')}</span>
    {/if}
  </label>
  <p class="text-xs text-base-content/60 mt-1.5">{$t(savedCents > 0 ? 'runLimit.onHint' : 'runLimit.offHint')}</p>
</div>
