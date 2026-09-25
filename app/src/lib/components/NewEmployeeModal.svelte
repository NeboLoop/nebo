<!--
  NewEmployeeModal — hire an additional employee. Same doctrine as the
  christening: hiring starts with a name. Unlike christening this is
  dismissible — the workforce already exists.

  An optional job description is worked out into what the employee will be
  able to do (the one needs step, `POST /agents/needs`), shown above Create
  in plain words. Each item is removable; Create grants what is left.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { X } from 'lucide-svelte';
  import { createAgent, workOutAgentNeeds } from '$lib/api/nebo';

  let { onclose, oncreated }: {
    onclose: () => void;
    oncreated: (agentId: string, name: string, threadId: string | null) => void;
  } = $props();

  let name = $state('');
  let job = $state('');
  let busy = $state(false);
  let errorMsg = $state('');

  // The drafted job: its plain-words items and the draft Create grants.
  let draftId = $state<string | null>(null);
  let items = $state<string[]>([]);
  let removed = $state<string[]>([]);
  let working = $state(false);
  let asked = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const valid = $derived(name.trim().length > 0 && name.trim().length <= 40);
  const kept = $derived(items.filter((i) => !removed.includes(i)));

  async function workOut() {
    const description = job.trim();
    const mine = ++asked;
    if (!description) {
      draftId = null;
      items = [];
      removed = [];
      working = false;
      return;
    }
    working = true;
    try {
      const res = await workOutAgentNeeds({ name: name.trim(), description });
      if (mine !== asked) return;
      draftId = res.draftId;
      items = res.items;
      removed = removed.filter((r) => res.items.includes(r));
    } catch {
      if (mine === asked) {
        draftId = null;
        items = [];
      }
    }
    if (mine === asked) working = false;
  }

  // Worked out once the owner pauses, not on every keystroke.
  function jobChanged() {
    clearTimeout(timer);
    timer = setTimeout(workOut, 700);
  }

  function remove(item: string) {
    removed = [...removed, item];
  }

  async function create() {
    if (!valid || busy || working) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createAgent({
        blank: true,
        name: name.trim(),
        ...(draftId ? { draftId, removed } : {}),
      });
      oncreated(resp.agent.id, resp.agent.name, resp.threadId);
    } catch (e: unknown) {
      errorMsg = e instanceof Error ? e.message : $t('newEmployee.failed');
      busy = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      create();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    }
  }
</script>

<div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
  <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" role="presentation" onclick={() => !busy && onclose()}></div>
  <div class="relative w-full max-w-sm rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-6 flex flex-col items-center text-center">
    <h1 class="text-base font-semibold">{$t('newEmployee.title')}</h1>
    <p class="text-sm text-base-content/70 mt-2 leading-relaxed">{$t('newEmployee.lede')}</p>

    <div class="w-full mt-5 flex flex-col gap-2">
      <input
        type="text"
        class="input input-bordered w-full text-center text-lg font-medium"
        placeholder={$t('newEmployee.placeholder')}
        maxlength="40"
        bind:value={name}
        {onkeydown}
        autofocus
      />
      <textarea
        class="textarea textarea-bordered w-full text-sm"
        rows="2"
        placeholder={$t('newEmployee.jobPlaceholder')}
        aria-label={$t('newEmployee.jobLabel')}
        bind:value={job}
        oninput={jobChanged}
      ></textarea>
      {#if working}
        <p class="consent-chip-hint">{$t('newEmployee.working')}</p>
      {:else if kept.length}
        <div class="w-full flex flex-col gap-1.5 text-left">
          <p class="consent-line">{$t('newEmployee.willDo', { values: { name: name.trim() || $t('newEmployee.thisEmployee') } })}</p>
          <div class="consent-items">
            {#each kept as item (item)}
              <span class="consent-item">
                {item}
                <button type="button" class="consent-item-remove" aria-label={$t('newEmployee.removeItem', { values: { item } })} onclick={() => remove(item)}>
                  <X class="w-3 h-3" />
                </button>
              </span>
            {/each}
          </div>
        </div>
      {/if}
      {#if errorMsg}
        <p class="text-xs text-error">{errorMsg}</p>
      {/if}
    </div>

    <div class="w-full mt-4 flex gap-2">
      <button type="button" class="btn btn-ghost rounded-field flex-1" onclick={onclose} disabled={busy}>
        {$t('common.cancel')}
      </button>
      <button type="button" class="btn btn-primary rounded-field flex-1" disabled={!valid || busy || working} onclick={create}>
        {#if busy}
          <span class="loading loading-spinner loading-xs"></span>
        {:else}
          {$t('newEmployee.create')}
        {/if}
      </button>
    </div>

    <p class="text-xs text-base-content/50 mt-4">{$t('newEmployee.marketplaceHint')}</p>
  </div>
</div>
