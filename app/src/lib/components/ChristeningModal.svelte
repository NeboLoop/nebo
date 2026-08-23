<!--
  ChristeningModal — the first-contact ceremony. On first start (after
  onboarding; cloud installs hit it on first workspace mount), the owner NAMES
  their first employee. Not skippable by design: no close button, no backdrop
  dismiss, no Escape — naming is the hiring moment, not a setting.

  Flow: name → watch the employee get created (staged reveal) → land in the
  first thread where it introduces itself live.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { christenPrimary } from '$lib/api/nebo';

  let { oncreated }: { oncreated: (threadId: string, name: string) => void } = $props();

  let name = $state('');
  let phase = $state<'naming' | 'creating' | 'error'>('naming');
  let errorMsg = $state('');

  const valid = $derived(name.trim().length > 0 && name.trim().length <= 40);
  const initial = $derived((name.trim()[0] ?? '?').toUpperCase());

  async function create() {
    if (!valid || phase === 'creating') return;
    phase = 'creating';
    const started = Date.now();
    try {
      const resp = await christenPrimary({ name: name.trim() });
      // Let the creation moment breathe — the reveal is the point.
      const elapsed = Date.now() - started;
      if (elapsed < 1800) await new Promise((r) => setTimeout(r, 1800 - elapsed));
      oncreated(resp.threadId, resp.name);
    } catch (e: unknown) {
      phase = 'error';
      errorMsg = e instanceof Error ? e.message : $t('christen.failed');
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      create();
    }
  }
</script>

<!-- Deliberately NOT ShelfModal: this overlay has no close affordance at all. -->
<div class="fixed inset-0 z-[90] flex items-center justify-center p-4 bg-base-300/80 backdrop-blur-sm">
  <div class="w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-8 flex flex-col items-center text-center">
    {#if phase !== 'creating'}
      <h1 class="text-base font-semibold">{$t('christen.title')}</h1>
      <p class="text-sm text-base-content/70 mt-2 leading-relaxed">{$t('christen.lede')}</p>

      <div class="w-full mt-6 flex flex-col gap-2">
        <input
          type="text"
          class="input input-bordered w-full text-center text-lg font-medium"
          placeholder={$t('christen.placeholder')}
          maxlength="40"
          bind:value={name}
          {onkeydown}
          autofocus
        />
        {#if phase === 'error'}
          <p class="text-xs text-error">{errorMsg}</p>
        {/if}
      </div>

      <button
        type="button"
        class="btn btn-primary rounded-field w-full mt-4"
        disabled={!valid}
        onclick={create}
      >
        {$t('christen.create')}
      </button>
    {:else}
      <!-- The creation moment: the avatar assembles, the name settles in. -->
      <div class="py-6 flex flex-col items-center gap-4 motion-safe:animate-[christen-in_0.5s_ease-out]">
        <div class="relative">
          <div class="w-20 h-20 rounded-field bg-primary/10 text-primary flex items-center justify-center font-mono text-3xl font-semibold motion-safe:animate-[christen-pulse_1.2s_ease-in-out_infinite]">
            {initial}
          </div>
          <div class="absolute -bottom-1 -right-1 w-4 h-4 rounded-full border-2 border-base-100 bg-success motion-safe:animate-[christen-pulse_1.2s_ease-in-out_infinite]"></div>
        </div>
        <div class="text-lg font-semibold">{name.trim()}</div>
        <div class="flex items-center gap-2 text-sm text-base-content/60">
          <span class="loading loading-spinner loading-xs text-primary"></span>
          {$t('christen.creating', { values: { name: name.trim() } })}
        </div>
      </div>
    {/if}
  </div>
</div>
