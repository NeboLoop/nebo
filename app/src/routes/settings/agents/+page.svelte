<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import type { Agent } from '$lib/api/nebo';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import StatCard from '$lib/components/settings/StatCard.svelte';
  import SettingsRow from '$lib/components/settings/SettingsRow.svelte';
  import BrowseCard from '$lib/components/settings/BrowseCard.svelte';
  import ManageModal from '$lib/components/settings/ManageModal.svelte';
  import ConfirmModal from '$lib/components/settings/ConfirmModal.svelte';

  type AgentRow = { id: string; name: string; role: string; status: string };

  let agents = $state<AgentRow[]>([]);
  let search = $state('');
  let selected = $state<AgentRow | null>(null);
  let confirming = $state(false);
  let removing = $state(false);

  const onlineCount = $derived(agents.filter((a) => a.status === 'online').length);
  const filtered = $derived.by(() => {
    const q = search.trim().toLowerCase();
    if (!q) return agents;
    return agents.filter((a) => a.name.toLowerCase().includes(q) || a.role.toLowerCase().includes(q));
  });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        agents = (resp.agents as Agent[]).map((a) => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          status: a.isEnabled ? 'online' : 'paused',
        }));
      }
    } catch { /* leave list empty on error */ }
  });

  async function uninstall() {
    if (!selected) return;
    removing = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteAgent(selected.id);
      agents = agents.filter((a) => a.id !== selected!.id);
      selected = null;
      confirming = false;
    } catch {
      removing = false;
    }
  }
</script>

<SettingsHeader title={$t('settingsAgents.pageTitle')} description={$t('settingsAgents.pageDescription')} />

<div class="flex gap-3 mb-6">
  <StatCard label={$t('settingsAgents.statAgents')} value={agents.length} />
  <StatCard label={$t('common.online')} value={onlineCount} accent="success" />
</div>

<div class="mb-6">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-base font-semibold">{$t('settingsAgents.installedEmployees')}</h3>
    {#if agents.length > 0}
      <input type="text" bind:value={search} placeholder={$t('settingsAgents.searchPlaceholder')} class="input input-sm input-bordered max-w-xs text-sm" />
    {/if}
  </div>

  {#if filtered.length === 0}
    <div class="text-center py-8">
      <div class="text-xs text-base-content/50">{search ? $t('settingsAgents.noMatch', { values: { search } }) : $t('settingsAgents.noneInstalled')}</div>
    </div>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each filtered as agent}
        <SettingsRow>
          {#snippet leading()}
            <div
              class="w-2 h-2 rounded-full shrink-0 {agent.status === 'online' ? 'bg-success' : 'bg-base-content/20'}"
              title={agent.status === 'online' ? $t('common.online') : $t('common.paused')}
            ></div>
          {/snippet}
          <button class="text-sm font-semibold text-primary hover:underline cursor-pointer bg-transparent border-none p-0 text-left" onclick={() => { selected = agent; confirming = false; }}>{agent.name}</button>
          {#if agent.role}
            <div class="text-xs text-base-content/70 line-clamp-1">{agent.role}</div>
          {/if}
          {#snippet actions()}
            <a href="/{agent.id}/settings" class="px-3 py-1 rounded-md border border-base-300 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors no-underline">{$t('settingsAgents.configure')}</a>
          {/snippet}
        </SettingsRow>
      {/each}
    </div>
  {/if}
</div>

<BrowseCard
  title={$t('settingsAgents.browseTitle')}
  description={$t('settingsAgents.browseDescription')}
  href="/marketplace/agents"
/>

{#if selected}
  <ManageModal
    title={selected.name}
    subtitle={selected.status}
    onClose={() => (selected = null)}
  >
    {#if selected.role}
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('common.description')}</div>
        <p class="text-xs text-base-content/70">{selected.role}</p>
      </div>
    {/if}
    <div>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('common.status')}</div>
      <span class="px-2 py-0.5 rounded text-xs font-medium {selected.status === 'online' ? 'bg-success/10 text-success' : 'bg-base-200 text-base-content/60'}">{selected.status}</span>
    </div>
    {#snippet footer()}
      <a href="/{selected?.id}/settings" class="px-3 py-1.5 rounded-md border border-base-300 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors no-underline">{$t('settingsAgents.configure')}</a>
      <button class="px-3 py-1.5 rounded-md border border-error/30 text-xs text-error font-medium cursor-pointer bg-transparent hover:bg-error/5 transition-colors" onclick={() => (confirming = true)}>{$t('common.uninstall')}</button>
    {/snippet}
  </ManageModal>

  {#if confirming}
    <ConfirmModal
      title={$t('common.uninstallTitle', { values: { name: selected.name } })}
      message={$t('settingsAgents.uninstallMessage')}
      confirmLabel={$t('common.uninstall')}
      busy={removing}
      onCancel={() => (confirming = false)}
      onConfirm={uninstall}
    />
  {/if}
{/if}
