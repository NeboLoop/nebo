<!--
  NewProjectModal — the team page's "+ New project": a title and what to
  do. Starting it opens the project's own thread with the ask as its first
  post; the team's lead takes it from there. A project has a lead who owns
  it, so a team without one is asked to pick its lead first (the team's
  edit, `onpicklead`).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { createProject } from '$lib/api/nebo';
  import type { Project, Team } from '$lib/api/neboComponents';

  let { team, onclose, oncreated, onpicklead }: {
    team: Team;
    onclose: () => void;
    /** Called with the started project. */
    oncreated: (project: Project) => void;
    /** Open the team's edit to pick its lead. */
    onpicklead: () => void;
  } = $props();

  let title = $state('');
  let ask = $state('');
  let busy = $state(false);
  let errorMsg = $state('');

  // The server's rule (`lead_of`): a lead who is on the team.
  const hasLead = $derived(!!team.organizerAgentId && team.members.some((m) => m.agentId === team.organizerAgentId));
  const valid = $derived(hasLead && ask.trim().length > 0);

  async function start() {
    if (!valid || busy) return;
    busy = true;
    errorMsg = '';
    try {
      const resp = await createProject(team.id, { title: title.trim(), ask: ask.trim() });
      oncreated(resp.project);
    } catch (e: unknown) {
      errorMsg = e instanceof Error && e.message ? e.message : $t('teams.projectCreateFailed');
      busy = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      start();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (!busy) onclose();
    }
  }
</script>

<div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true" tabindex="-1" onkeydown={onkeydown}>
  <div class="absolute inset-0 bg-black/50 backdrop-blur-sm" role="presentation" onclick={() => !busy && onclose()}></div>
  <div class="relative w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl flex flex-col max-h-[85vh]">
    <div class="px-6 pt-6 pb-3">
      <h1 class="text-base font-semibold">{$t('teams.newProjectTitle')}</h1>
      <p class="text-sm text-base-content/70 mt-1 leading-relaxed">{$t('teams.newProjectLede')}</p>
    </div>

    {#if hasLead}
      <div class="px-6 flex flex-col gap-2 min-h-0 overflow-y-auto">
        <input
          class="input input-bordered input-sm w-full"
          placeholder={$t('teams.projectTitlePlaceholder')}
          bind:value={title}
          maxlength="60"
          aria-label={$t('teams.projectTitleLabel')}
        />
        <textarea
          class="textarea textarea-bordered w-full min-h-28 text-sm leading-relaxed"
          placeholder={$t('teams.projectAskPlaceholder')}
          bind:value={ask}
          aria-label={$t('teams.projectAskLabel')}
        ></textarea>
        {#if errorMsg}
          <p class="text-xs text-error">{errorMsg}</p>
        {/if}
      </div>
    {:else}
      <div class="px-6 flex flex-col items-start gap-2">
        <p class="text-sm text-base-content/80 leading-relaxed">{$t('teams.projectNeedsLead')}</p>
        <button class="btn btn-sm btn-outline" onclick={onpicklead}>{$t('teams.pickLead')}</button>
      </div>
    {/if}

    <div class="flex items-center justify-end gap-2 px-6 py-4">
      <button class="btn btn-ghost btn-sm" onclick={onclose} disabled={busy}>{$t('common.cancel')}</button>
      {#if hasLead}
        <button class="btn btn-primary btn-sm" onclick={start} disabled={!valid || busy}>
          {#if busy}<span class="loading loading-spinner loading-xs"></span>{/if}
          {$t('teams.projectCreate')}
        </button>
      {/if}
    </div>
  </div>
</div>
