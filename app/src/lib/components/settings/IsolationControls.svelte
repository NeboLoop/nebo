<!--
  IsolationControls — the per-employee memory mode (Settings → employee →
  General, below Self-Improvement). One choice, three options, same segmented
  visual language as LearningControls:

    One conversation       — a single running thread, one memory (default)
    Separate conversations — many conversations sharing one memory; a
                             conversation with someone else stays sealed
    Confidential           — many conversations, each a sealed matter; only
                             local memory reaches every one of them

  Reads `memoryMode` from the agent detail (the server reads it the way the
  runtime enforces it); writes ride the ONE canonical pathway — the agent PUT
  `memoryMode` field, which lands in agent.json's memory.mode on disk and in
  the DB.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import MessageSquare from 'lucide-svelte/icons/message-square';
  import MessagesSquare from 'lucide-svelte/icons/messages-square';
  import Lock from 'lucide-svelte/icons/lock';
  import * as api from '$lib/api/nebo';
  import ConfirmModal from './ConfirmModal.svelte';

  type MemoryMode = 'single' | 'separate' | 'confidential';

  let { agentId }: { agentId: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let mode = $state<MemoryMode>('single');
  // A phone line keeps conversations separate (callers must never share
  // memory); the server refuses one conversation while one is attached, so
  // the control says so.
  let phoneLocked = $state(false);

  const options: { value: MemoryMode; labelKey: string; hintKey: string; icon: typeof Lock; active: string }[] = [
    {
      value: 'single',
      labelKey: 'agentIsolation.single',
      hintKey: 'agentIsolation.singleHint',
      icon: MessageSquare,
      active: 'bg-success/15 border-success/40 text-success',
    },
    {
      value: 'separate',
      labelKey: 'agentIsolation.separate',
      hintKey: 'agentIsolation.separateHint',
      icon: MessagesSquare,
      active: 'bg-info/15 border-info/40 text-info',
    },
    {
      value: 'confidential',
      labelKey: 'agentIsolation.confidential',
      hintKey: 'agentIsolation.confidentialHint',
      icon: Lock,
      active: 'bg-warning/15 border-warning/40 text-warning',
    },
  ];

  const currentHint = $derived(options.find((o) => o.value === mode)?.hintKey ?? '');

  async function load() {
    loading = true;
    try {
      const resp = await api.getAgent(agentId);
      const m = resp.memoryMode;
      mode = m === 'separate' || m === 'confidential' ? m : 'single';
      try {
        const lines = (await api.neboAIPhoneLines()) as { numbers?: { agentId?: string; status?: string }[] };
        phoneLocked = (lines.numbers ?? []).some((l) => l.agentId === agentId && l.status === 'active');
      } catch {
        phoneLocked = false;
      }
    } catch {
      mode = 'single';
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (agentId) load();
  });

  // Going back to one conversation changes what the owner sees and what the
  // employee remembers — that deserves a real confirmation, not a silent
  // toggle.
  let confirmSingle = $state(false);

  function requestMode(value: MemoryMode) {
    if (saving || value === mode || (phoneLocked && value === 'single')) return;
    if (value === 'single') {
      confirmSingle = true;
      return;
    }
    setMode(value);
  }

  async function setMode(value: MemoryMode) {
    if (saving || value === mode) return;
    const prev = mode;
    mode = value;
    saving = true;
    try {
      await api.updateAgent(agentId, { memoryMode: value });
    } catch {
      mode = prev;
    } finally {
      saving = false;
    }
  }
</script>

<div class="max-w-2xl">
  {#if !loading}
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('agentIsolation.title')}</div>
    <div class="join">
      {#each options as o (o.value)}
        <button
          class="join-item flex items-center gap-1.5 px-3 py-1.5 text-xs border transition-colors cursor-pointer {mode === o.value
            ? o.active
            : 'bg-base-100 border-base-content/10 text-base-content/40 hover:text-base-content/70 hover:bg-base-200'}"
          aria-pressed={mode === o.value}
          disabled={saving || (phoneLocked && o.value === 'single')}
          onclick={() => requestMode(o.value)}
        >
          <o.icon class="w-3.5 h-3.5" />{$t(o.labelKey)}
        </button>
      {/each}
    </div>
    <p class="text-xs text-base-content/60 mt-1.5">{$t(currentHint)}</p>
    {#if phoneLocked}
      <p class="text-xs text-base-content/60 mt-1">{$t('agentIsolation.lockedByPhone')}</p>
    {/if}
  {/if}
</div>

{#if confirmSingle}
  <ConfirmModal
    title={$t('agentIsolation.confirmOffTitle')}
    message={$t('agentIsolation.confirmOffBody')}
    confirmLabel={$t('agentIsolation.confirmOffAction')}
    busy={saving}
    onConfirm={() => { confirmSingle = false; setMode('single'); }}
    onCancel={() => (confirmSingle = false)}
  />
{/if}
