<!--
  IsolationControls — the per-employee memory isolation toggle (Settings →
  employee → General, below Self-Improvement). Two states, same segmented
  visual language as LearningControls:

    Shared memory       — one memory across every conversation (default)
    Isolated per matter — each conversation keeps its own sealed memory;
                          nothing bleeds between cases/clients/engagements

  Reads the agent's frontmatter memory config; writes ride the ONE canonical
  pathway — the agent PUT `contextIsolated` field, which merges into
  agent.json's memory.context_isolated on disk and in the DB.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Users from 'lucide-svelte/icons/users';
  import Lock from 'lucide-svelte/icons/lock';
  import * as api from '$lib/api/nebo';
  import ConfirmModal from './ConfirmModal.svelte';

  let { agentId }: { agentId: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let isolated = $state(false);

  const options: { value: boolean; labelKey: string; hintKey: string; icon: typeof Lock; active: string }[] = [
    {
      value: false,
      labelKey: 'agentIsolation.shared',
      hintKey: 'agentIsolation.sharedHint',
      icon: Users,
      active: 'bg-success/15 border-success/40 text-success',
    },
    {
      value: true,
      labelKey: 'agentIsolation.isolated',
      hintKey: 'agentIsolation.isolatedHint',
      icon: Lock,
      active: 'bg-warning/15 border-warning/40 text-warning',
    },
  ];

  const currentHint = $derived(options.find((o) => o.value === isolated)?.hintKey ?? '');

  async function load() {
    loading = true;
    try {
      const resp = (await api.getAgent(agentId)) as { agent?: { frontmatter?: string } };
      const fm = JSON.parse(resp.agent?.frontmatter || '{}');
      isolated = fm?.memory?.context_isolated === true;
    } catch {
      isolated = false;
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (agentId) load();
  });

  // Turning isolation OFF changes what the owner sees and what the employee
  // remembers — that deserves a real confirmation, not a silent toggle.
  let confirmOff = $state(false);

  function requestIsolated(value: boolean) {
    if (saving || value === isolated) return;
    if (!value && isolated) {
      confirmOff = true;
      return;
    }
    setIsolated(value);
  }

  async function setIsolated(value: boolean) {
    if (saving || value === isolated) return;
    const prev = isolated;
    isolated = value;
    saving = true;
    try {
      await api.updateAgent(agentId, { contextIsolated: value });
    } catch {
      isolated = prev;
    } finally {
      saving = false;
    }
  }
</script>

<div class="max-w-2xl">
  {#if !loading}
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('agentIsolation.title')}</div>
    <div class="join">
      {#each options as o (String(o.value))}
        <button
          class="join-item flex items-center gap-1.5 px-3 py-1.5 text-xs border transition-colors cursor-pointer {isolated === o.value
            ? o.active
            : 'bg-base-100 border-base-content/10 text-base-content/40 hover:text-base-content/70 hover:bg-base-200'}"
          aria-pressed={isolated === o.value}
          disabled={saving}
          onclick={() => requestIsolated(o.value)}
        >
          <o.icon class="w-3.5 h-3.5" />{$t(o.labelKey)}
        </button>
      {/each}
    </div>
    <p class="text-xs text-base-content/60 mt-1.5">{$t(currentHint)}</p>
  {/if}
</div>

{#if confirmOff}
  <ConfirmModal
    title={$t('agentIsolation.confirmOffTitle')}
    message={$t('agentIsolation.confirmOffBody')}
    confirmLabel={$t('agentIsolation.confirmOffAction')}
    busy={saving}
    onConfirm={() => { confirmOff = false; setIsolated(false); }}
    onCancel={() => (confirmOff = false)}
  />
{/if}
