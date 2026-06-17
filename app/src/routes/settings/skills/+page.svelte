<script lang="ts">
  import { onMount } from 'svelte';
  import type { ExtensionInfo } from '$lib/api/nebo';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import StatCard from '$lib/components/settings/StatCard.svelte';
  import SettingsRow from '$lib/components/settings/SettingsRow.svelte';
  import BrowseCard from '$lib/components/settings/BrowseCard.svelte';
  import ManageModal from '$lib/components/settings/ManageModal.svelte';
  import ConfirmModal from '$lib/components/settings/ConfirmModal.svelte';

  let skills = $state<ExtensionInfo[]>([]);
  let search = $state('');
  let selected = $state<ExtensionInfo | null>(null);
  let confirming = $state(false);
  let removing = $state(false);

  const enabledCount = $derived(skills.filter((s) => s.enabled).length);
  const sourceCount = $derived(new Set(skills.map((s) => s.source)).size);
  const filtered = $derived.by(() => {
    const sorted = [...skills].sort((a, b) => a.name.localeCompare(b.name));
    const q = search.trim().toLowerCase();
    if (!q) return sorted;
    return sorted.filter((s) => s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q));
  });

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listExtensions();
      skills = resp.extensions;
    } catch { /* leave list empty on error */ }
  });

  async function toggleSkill(skill: ExtensionInfo) {
    skill.enabled = !skill.enabled;
    try {
      const api = await import('$lib/api/nebo');
      await api.toggleSkill(skill.name);
    } catch { /* local state already updated */ }
  }

  async function uninstall() {
    if (!selected) return;
    removing = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteSkill(selected.name);
      skills = skills.filter((s) => s.name !== selected!.name);
      selected = null;
      confirming = false;
    } catch {
      /* keep the selection so the user can retry */
    } finally {
      removing = false;
    }
  }
</script>

<SettingsHeader title="Skills" description="Manage installed skills and their capabilities." />

<div class="flex gap-3 mb-6">
  <StatCard label="Skills" value={skills.length} />
  <StatCard label="Enabled" value={enabledCount} accent="success" />
  <StatCard label="Sources" value={sourceCount} />
</div>

<div class="mb-6">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-base font-semibold">Installed Skills</h3>
    {#if skills.length > 0}
      <input type="text" bind:value={search} placeholder="Search skills…" class="input input-sm input-bordered max-w-xs text-sm" />
    {/if}
  </div>

  {#if filtered.length === 0}
    <div class="text-center py-8">
      <div class="text-xs text-base-content/50">{search ? `No skills match "${search}"` : 'No skills installed.'}</div>
    </div>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each filtered as skill}
        <SettingsRow>
          {#snippet leading()}
            <div
              class="w-2 h-2 rounded-full shrink-0 {skill.enabled ? 'bg-success' : 'bg-base-content/20'}"
              title={skill.enabled ? 'Enabled' : 'Disabled'}
            ></div>
          {/snippet}
          <div class="flex items-center gap-2 mb-0.5">
            <button class="text-sm font-semibold text-primary hover:underline cursor-pointer bg-transparent border-none p-0 text-left" onclick={() => { selected = skill; confirming = false; }}>{skill.name}</button>
            <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70">{skill.source}</span>
          </div>
          {#if skill.description}
            <div class="text-xs text-base-content/70 line-clamp-1">{skill.description}</div>
          {/if}
          {#snippet actions()}
            <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={skill.enabled} onchange={() => toggleSkill(skill)} />
          {/snippet}
        </SettingsRow>
      {/each}
    </div>
  {/if}
</div>

<BrowseCard
  title="Browse Skills"
  description="Discover more skills in the marketplace."
  href="/marketplace/skills"
/>

{#if selected}
  <ManageModal
    title={selected.name}
    subtitle={selected.source}
    leading={selected.name.charAt(0).toUpperCase()}
    onClose={() => (selected = null)}
  >
    {#if selected.description}
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Description</div>
        <p class="text-xs text-base-content/70">{selected.description}</p>
      </div>
    {/if}
    <div>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Status</div>
      <div class="flex items-center gap-2">
        <span class="px-2 py-0.5 rounded text-xs font-medium {selected.enabled ? 'bg-success/10 text-success' : 'bg-base-200 text-base-content/60'}">{selected.enabled ? 'Enabled' : 'Disabled'}</span>
        <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70">{selected.source}</span>
      </div>
    </div>
    {#if selected.capabilities.length > 0}
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Capabilities</div>
        <div class="flex flex-wrap gap-1">
          {#each selected.capabilities as capability}
            <span class="px-1.5 py-0.5 rounded bg-base-200 text-xs font-mono text-base-content/70">{capability}</span>
          {/each}
        </div>
      </div>
    {/if}
    {#snippet footer()}
      <button class="px-3 py-1.5 rounded-md border border-base-300 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={() => selected && toggleSkill(selected)}>{selected?.enabled ? 'Disable' : 'Enable'}</button>
      <button class="px-3 py-1.5 rounded-md border border-error/30 text-xs text-error font-medium cursor-pointer bg-transparent hover:bg-error/5 transition-colors" onclick={() => (confirming = true)}>Uninstall</button>
    {/snippet}
  </ManageModal>

  {#if confirming}
    <ConfirmModal
      title="Uninstall {selected.name}?"
      message="This removes the skill from this companion. You can reinstall it from the marketplace later."
      confirmLabel="Uninstall"
      busy={removing}
      onCancel={() => (confirming = false)}
      onConfirm={uninstall}
    />
  {/if}
{/if}
