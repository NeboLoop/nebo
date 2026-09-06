<!--
  NewTeamModal — put employees on a team in one step. Pick two or more,
  the name fills itself from the first two picks (editable), the mission is
  optional. Creating lands the owner in the team's conversation.

  With `team` set it is the SAME picker in edit mode: current members come
  checked, unchecking removes, name and mission are prefilled. One flow to
  learn; both doors go through the server's one rule set (two-member floor,
  organizer stays).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { untrack } from 'svelte';
  import { openTeam, editTeam } from '$lib/api/nebo';
  import type { Team } from '$lib/api/neboComponents';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';

  let { roster, team = null, onclose, oncreated }: {
    roster: { id: string; name: string; color?: string; isApp?: boolean }[];
    /** Edit this team instead of creating one. */
    team?: Team | null;
    onclose: () => void;
    /** Called with the created or updated team. */
    oncreated: (team: Team) => void;
  } = $props();

  // Apps are interfaces, not teammates.
  const candidates = $derived(
    roster.filter((a) => !a.isApp).sort((a, b) => a.name.localeCompare(b.name))
  );

  // The picker mounts fresh per open, so the team it edits is read once.
  const initial = untrack(() => team);
  let picked = $state<string[]>(initial ? [...initial.memberAgentIds] : []);
  let typedName = $state(initial?.name ?? '');
  let mission = $state(initial?.mission ?? '');
  // The lead: a member that gets posts addressed to nobody in particular.
  // '' = you lead. Picking an employee that is then unchecked falls back to you.
  let lead = $state(initial?.organizerAgentId ?? '');
  let busy = $state(false);
  let errorMsg = $state('');

  const suggestedName = $derived(
    picked
      .slice(0, 2)
      .map((id) => candidates.find((a) => a.id === id)?.name ?? '')
      .filter(Boolean)
      .join(' & ')
  );
  const name = $derived(typedName.trim() || suggestedName);
  const valid = $derived(picked.length >= 2 && name.length > 0 && name.length <= 60);

  function toggle(id: string) {
    picked = picked.includes(id) ? picked.filter((p) => p !== id) : [...picked, id];
    if (!picked.includes(lead)) lead = '';
  }

  async function create() {
    if (!valid || busy) return;
    busy = true;
    errorMsg = '';
    try {
      const body = { name, mission: mission.trim(), agentIds: picked, organizerAgentId: lead };
      const resp = team ? await editTeam(team.id, body) : await openTeam(body);
      oncreated(resp.team);
    } catch (e: unknown) {
      errorMsg = e instanceof Error ? e.message : $t('teams.createFailed');
      busy = false;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !(e.target instanceof HTMLTextAreaElement)) {
      e.preventDefault();
      create();
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
      <h1 class="text-base font-semibold">{team ? $t('teams.editTitle') : $t('teams.newTitle')}</h1>
      <p class="text-sm text-base-content/70 mt-1 leading-relaxed">{team ? $t('teams.editLede') : $t('teams.newLede')}</p>
    </div>

    <!-- Who: a checklist of employees, the same chips as the sidebar. -->
    <div class="px-3 min-h-0 overflow-y-auto">
      {#each candidates as a (a.id)}
        {@const on = picked.includes(a.id)}
        <label class="flex items-center gap-3 px-3 py-2 rounded-box cursor-pointer select-none {on ? 'bg-primary/10' : 'hover:bg-base-200'}">
          <input type="checkbox" class="checkbox checkbox-sm checkbox-primary" checked={on} onchange={() => toggle(a.id)} />
          <AgentAvatar name={a.name} color={a.color} size="sm" />
          <span class="text-sm truncate min-w-0 flex-1">{a.name}</span>
          {#if on}
            <!-- One lead per team; the choice only shows on checked rows. -->
            <label class="flex items-center gap-1.5 text-xs cursor-pointer shrink-0 {lead === a.id ? 'text-warning' : 'text-base-content/50'}" title={$t('teams.leadHint')}>
              <input type="radio" name="team-lead" class="radio radio-xs radio-warning" checked={lead === a.id} onchange={() => (lead = a.id)} onclick={(e) => { if (lead === a.id) { e.preventDefault(); lead = ''; } }} />
              {$t('teams.lead')}
            </label>
          {/if}
        </label>
      {/each}
      {#if candidates.length < 2}
        <p class="text-xs text-base-content/50 px-3 py-2">{$t('teams.needTwoEmployees')}</p>
      {/if}
    </div>

    <div class="px-6 pt-3 flex flex-col gap-2">
      <input
        class="input input-bordered input-sm w-full"
        placeholder={suggestedName || $t('teams.namePlaceholder')}
        bind:value={typedName}
        maxlength="60"
        aria-label={$t('teams.nameLabel')}
      />
      <input
        class="input input-bordered input-sm w-full"
        placeholder={$t('teams.missionPlaceholder')}
        bind:value={mission}
        maxlength="300"
        aria-label={$t('teams.missionLabel')}
      />
      {#if errorMsg}
        <p class="text-xs text-error">{errorMsg}</p>
      {/if}
    </div>

    <div class="flex items-center justify-between gap-2 px-6 py-4">
      <span class="text-xs text-base-content/50">{$t('teams.pickedCount', { values: { count: picked.length } })}{#if !lead} · {$t('teams.youLead')}{/if}</span>
      <div class="flex items-center gap-2">
        <button class="btn btn-ghost btn-sm" onclick={onclose} disabled={busy}>{$t('common.cancel')}</button>
        <button class="btn btn-primary btn-sm" onclick={create} disabled={!valid || busy}>
          {#if busy}<span class="loading loading-spinner loading-xs"></span>{/if}
          {team ? $t('teams.save') : $t('teams.create')}
        </button>
      </div>
    </div>
  </div>
</div>
