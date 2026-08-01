<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import Search from 'lucide-svelte/icons/search';
  import Download from 'lucide-svelte/icons/download';
  import FolderOpen from 'lucide-svelte/icons/folder-open';
  import TriangleAlert from 'lucide-svelte/icons/triangle-alert';
  import CircleCheck from 'lucide-svelte/icons/circle-check';
  import {
    detectInstalls,
    scanInstall,
    applyInstall,
    type DetectedInstall,
    type ImportManifest,
    type ImportItem,
    type ImportOutcome,
  } from '$lib/api/import';

  let detected = $state<DetectedInstall[]>([]);
  let path = $state('');
  let manifest = $state<ImportManifest | null>(null);
  let outcome = $state<ImportOutcome | null>(null);
  let scanning = $state(false);
  let importing = $state(false);
  let error = $state('');

  const KIND_ORDER: ImportItem['kind'][] = [
    'agent',
    'skill',
    'mcp_server',
    'memory',
    'session',
    'cron',
    'credential',
  ];

  const grouped = $derived.by(() => {
    if (!manifest) return [];
    return KIND_ORDER.map((kind) => ({
      kind,
      items: manifest!.items.filter((i) => i.kind === kind),
    })).filter((g) => g.items.length > 0);
  });

  onMount(async () => {
    try {
      const resp = await detectInstalls();
      detected = resp.installs;
      const first = detected.find((d) => d.importable);
      if (first) path = first.path;
    } catch {
      /* detection is best-effort; manual path entry still works */
    }
  });

  async function scan(target?: string) {
    if (target) path = target;
    if (!path.trim()) return;
    scanning = true;
    error = '';
    outcome = null;
    manifest = null;
    try {
      const resp = await scanInstall(path.trim());
      manifest = resp.manifest;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      scanning = false;
    }
  }

  async function runImport() {
    if (!manifest) return;
    importing = true;
    error = '';
    try {
      const resp = await applyInstall(manifest.root);
      outcome = resp.outcome;
      manifest = null;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      importing = false;
    }
  }
</script>

<SettingsHeader title={$t('settingsImport.title')} description={$t('settingsImport.description')} />

{#if detected.length > 0}
  <div class="mb-5">
    <h3 class="text-sm font-semibold mb-2">{$t('settingsImport.detectedTitle')}</h3>
    <div class="flex flex-col gap-2">
      {#each detected as install}
        <div class="flex items-center gap-3 py-2.5 px-3 rounded-lg border border-base-content/10">
          <FolderOpen class="w-4 h-4 text-base-content/70" />
          <div class="flex-1 min-w-0">
            <span class="text-sm font-medium capitalize">{install.source}</span>
            <span class="text-xs text-base-content/70 ml-2 truncate">{install.path}</span>
          </div>
          {#if install.importable}
            <button
              class="btn btn-sm btn-primary"
              disabled={scanning}
              onclick={() => scan(install.path)}
            >
              <Search class="w-3.5 h-3.5" />
              {$t('settingsImport.scan')}
            </button>
          {:else}
            <span class="badge badge-ghost badge-sm">{$t('settingsImport.comingSoon')}</span>
          {/if}
        </div>
      {/each}
    </div>
  </div>
{/if}

<div class="mb-5">
  <h3 class="text-sm font-semibold mb-2">{$t('settingsImport.pathTitle')}</h3>
  <div class="flex gap-2">
    <input
      type="text"
      class="input input-bordered input-sm flex-1 font-mono"
      placeholder={$t('settingsImport.pathPlaceholder')}
      bind:value={path}
      onkeydown={(e) => e.key === 'Enter' && scan()}
    />
    <button class="btn btn-sm" disabled={scanning || !path.trim()} onclick={() => scan()}>
      <Search class="w-3.5 h-3.5" />
      {scanning ? $t('settingsImport.scanning') : $t('settingsImport.scan')}
    </button>
  </div>
</div>

{#if error}
  <div class="alert alert-error text-sm mb-5">{error}</div>
{/if}

{#if manifest}
  <div class="mb-5">
    <h3 class="text-sm font-semibold mb-2">
      {$t('settingsImport.manifestTitle', { values: { source: manifest.source } })}
    </h3>
    <div class="flex flex-col gap-4">
      {#each grouped as group}
        <div>
          <h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">
            {$t(`settingsImport.kinds.${group.kind}`)}
            <span class="ml-1">({group.items.length})</span>
          </h4>
          <div class="flex flex-col gap-1">
            {#each group.items as item}
              <div class="flex items-center gap-2.5 py-1.5 px-3 rounded-lg hover:bg-base-200/50">
                <span class="text-sm font-medium">{item.name}</span>
                <span class="text-xs text-base-content/70 flex-1 truncate">{item.detail}</span>
                {#if item.tier === 'code'}
                  <span class="badge badge-warning badge-sm gap-1">
                    <TriangleAlert class="w-3 h-3" />
                    {$t('settingsImport.runsCode')}
                  </span>
                {/if}
                <span class="text-xs text-base-content/50">→ {item.target}</span>
              </div>
            {/each}
          </div>
        </div>
      {/each}
    </div>
    {#if manifest.notes.length > 0}
      <div class="mt-3 text-xs text-base-content/70">
        {#each manifest.notes as note}
          <div>{note}</div>
        {/each}
      </div>
    {/if}
    <div class="mt-4">
      <button class="btn btn-primary btn-sm" disabled={importing} onclick={runImport}>
        <Download class="w-3.5 h-3.5" />
        {importing ? $t('settingsImport.importing') : $t('settingsImport.importAll')}
      </button>
    </div>
  </div>
{/if}

{#if outcome}
  <div class="mb-5">
    <div class="alert alert-success text-sm mb-3">
      <CircleCheck class="w-4 h-4" />
      {$t('settingsImport.doneSummary', {
        values: {
          agents: outcome.agents,
          skills: outcome.skills,
          mcp: outcome.mcpServers,
          keys: outcome.authProfiles,
        },
      })}
    </div>
    {#if outcome.skipped.length > 0}
      <h4 class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">
        {$t('settingsImport.skippedTitle')}
      </h4>
      <div class="flex flex-col gap-0.5 text-xs text-base-content/70">
        {#each outcome.skipped as line}
          <div>{line}</div>
        {/each}
      </div>
    {/if}
  </div>
{/if}
