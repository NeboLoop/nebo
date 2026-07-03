<script lang="ts">
  import { onMount } from 'svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { addToast } from '$lib/stores/toast';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import CheckCircle2 from 'lucide-svelte/icons/check-circle-2';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';

  interface Pending {
    artifactId: string;
    artifactType: string;
    name?: string;
    localVersion: string;
    remoteVersion: string;
    autoUpdate: boolean;
    lastCheckedAt: number;
  }
  interface HistoryEntry {
    id: number;
    artifactId: string;
    artifactType: string;
    name: string;
    fromVersion: string;
    toVersion: string;
    status: string;
    detail: string;
    appliedAt: number;
  }
  interface UpdateSettings { agents: boolean; skills: boolean; plugins: boolean; connectors: boolean; checkIntervalHours: number }

  let pending = $state<Pending[]>([]);
  let history = $state<HistoryEntry[]>([]);
  let settings = $state<UpdateSettings>({ agents: true, skills: true, plugins: true, connectors: true, checkIntervalHours: 6 });
  let checking = $state(false);
  let updatingAll = $state(false);
  // artifactIds currently mid-apply (button → spinner)
  let applying = $state<Record<string, boolean>>({});

  async function loadAll() {
    try {
      const api = await import('$lib/api/nebo');
      const [u, s, h] = await Promise.all([
        api.listUpdates().catch(() => ({ updates: [] })),
        api.getUpdateSettings().catch(() => null),
        api.listUpdateHistory().catch(() => ({ history: [] })),
      ]);
      pending = (u as { updates: Pending[] })?.updates ?? [];
      history = (h as { history: HistoryEntry[] })?.history ?? [];
      if (s) settings = s as UpdateSettings;
    } catch { /* keep state */ }
  }

  onMount(loadAll);

  // Detection + apply results arrive over the WebSocket — refresh the lists, no polling.
  onWsEvent('artifact_updates_available', () => loadAll());
  onWsEvent<{ id: string }>('artifact_update_applied', (d) => { if (d?.id) applying = { ...applying, [d.id]: false }; loadAll(); });
  onWsEvent<{ id: string; error?: string }>('artifact_update_failed', (d) => {
    if (d?.id) applying = { ...applying, [d.id]: false };
    addToast(`Update failed: ${d?.error ?? 'unknown error'}`, 'error');
    loadAll();
  });

  async function checkNow() {
    checking = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.checkUpdates();
      addToast('Checking for updates…', 'info');
      // Results arrive via WS; give the check a moment then refresh as a fallback.
      setTimeout(loadAll, 4000);
    } catch { addToast('Could not start update check', 'error'); }
    finally { checking = false; }
  }

  async function update(p: Pending) {
    applying = { ...applying, [p.artifactId]: true };
    try {
      const api = await import('$lib/api/nebo');
      await api.applyUpdate(p.artifactId);
      addToast(`Updating ${p.artifactType} to ${p.remoteVersion}…`, 'info');
    } catch {
      applying = { ...applying, [p.artifactId]: false };
      addToast('Could not start update', 'error');
    }
  }

  // Update all: reuse the single-item apply for each pending update (Rule 1 — one
  // apply pathway). Completion streams back over WS and refreshes the list.
  async function updateAll() {
    updatingAll = true;
    try {
      for (const p of pending) {
        if (!applying[p.artifactId]) await update(p);
      }
    } finally {
      updatingAll = false;
    }
  }

  async function toggleAuto(p: Pending) {
    const enabled = !p.autoUpdate;
    p.autoUpdate = enabled;
    try {
      const api = await import('$lib/api/nebo');
      await api.setArtifactAutoUpdate(p.artifactId, { enabled });
    } catch { p.autoUpdate = !enabled; }
  }

  async function saveSettings() {
    try {
      const api = await import('$lib/api/nebo');
      await api.setUpdateSettings(settings as unknown as Record<string, unknown>);
    } catch { /* keep */ }
  }

  function rel(ts: number): string {
    if (!ts) return 'never';
    const d = Date.now() / 1000 - ts;
    if (d < 60) return 'just now';
    if (d < 3600) return `${Math.floor(d / 60)}m ago`;
    if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
    return `${Math.floor(d / 86400)}d ago`;
  }
</script>

<div class="flex items-center justify-between mb-1">
  <h2 class="text-lg font-semibold">Updates</h2>
  <button class="btn btn-sm btn-outline gap-2" onclick={checkNow} disabled={checking}>
    <RefreshCw class="w-4 h-4 {checking ? 'animate-spin' : ''}" />
    {checking ? 'Checking…' : 'Check now'}
  </button>
</div>
<p class="text-xs text-base-content/70 mb-6">Available updates for your installed plugins, agents, skills, MCP connectors, and apps. You approve each update; turn on auto-update per item to apply silently.</p>

<!-- Pending updates -->
<div class="flex items-center justify-between mb-3">
  <h3 class="text-sm font-semibold">Available updates</h3>
  {#if pending.length > 0}
    <button class="btn btn-xs btn-primary" onclick={updateAll} disabled={updatingAll}>
      {updatingAll ? 'Updating all…' : `Update all (${pending.length})`}
    </button>
  {/if}
</div>
{#if pending.length === 0}
  <div class="rounded-xl border border-base-300 px-4 py-6 text-center text-sm text-base-content/60 mb-8">
    Everything is up to date.
  </div>
{:else}
  <div class="divide-y divide-base-content/10 border border-base-300 rounded-xl mb-8">
    {#each pending as p (p.artifactType + ':' + p.artifactId)}
      <div class="flex items-center gap-3 px-4 py-3">
        <div class="flex-1 min-w-0">
          <div class="text-sm font-medium truncate">{p.name || p.artifactId}</div>
          <div class="text-xs text-base-content/60 flex items-center gap-1.5">
            <span class="capitalize">{p.artifactType}</span>
            <span>·</span>
            <span class="font-mono">{p.localVersion || '—'}</span>
            <ArrowRight class="w-3 h-3" />
            <span class="font-mono text-success">{p.remoteVersion}</span>
          </div>
        </div>
        <label class="flex items-center gap-1.5 text-xs text-base-content/70 cursor-pointer shrink-0">
          <input type="checkbox" class="toggle toggle-xs toggle-primary" checked={p.autoUpdate} onchange={() => toggleAuto(p)} />
          Auto
        </label>
        <button class="btn btn-xs btn-primary shrink-0" onclick={() => update(p)} disabled={applying[p.artifactId]}>
          {applying[p.artifactId] ? 'Updating…' : 'Update'}
        </button>
      </div>
    {/each}
  </div>
{/if}

<!-- Auto-check settings -->
<h3 class="text-sm font-semibold mb-3">Automatic checks</h3>
<div class="border border-base-300 rounded-xl divide-y divide-base-content/10 mb-8">
  {#each [['plugins', 'Plugins'], ['agents', 'Agents'], ['skills', 'Skills'], ['connectors', 'MCP Connectors']] as [key, label]}
    <div class="flex items-center justify-between px-4 py-3">
      <span class="text-sm">{label}</span>
      <input
        type="checkbox"
        class="toggle toggle-sm toggle-primary"
        checked={settings[key as 'plugins' | 'agents' | 'skills']}
        onchange={(e) => { settings[key as 'plugins' | 'agents' | 'skills'] = (e.currentTarget as HTMLInputElement).checked; saveSettings(); }}
      />
    </div>
  {/each}
  <div class="flex items-center justify-between px-4 py-3">
    <span class="text-sm">Check every</span>
    <select class="select select-sm select-bordered" bind:value={settings.checkIntervalHours} onchange={saveSettings}>
      <option value={1}>1 hour</option>
      <option value={6}>6 hours</option>
      <option value={24}>24 hours</option>
    </select>
  </div>
</div>

<!-- History -->
{#if history.length > 0}
  <h3 class="text-sm font-semibold mb-3">History</h3>
  <div class="divide-y divide-base-content/10 border border-base-300 rounded-xl mb-4">
    {#each history as h (h.id)}
      <div class="flex items-center gap-3 px-4 py-2.5">
        {#if h.status === 'applied'}
          <CheckCircle2 class="w-4 h-4 text-success shrink-0" />
        {:else}
          <AlertTriangle class="w-4 h-4 text-error shrink-0" />
        {/if}
        <div class="flex-1 min-w-0">
          <div class="text-sm truncate">{h.name || h.artifactId}</div>
          {#if h.status === 'failed' && h.detail}
            <div class="text-xs text-error/80 truncate">{h.detail}</div>
          {/if}
        </div>
        <span class="text-xs font-mono text-base-content/60 shrink-0">{h.fromVersion || '—'} → {h.toVersion}</span>
        <span class="text-xs text-base-content/50 shrink-0 w-16 text-right">{rel(h.appliedAt)}</span>
      </div>
    {/each}
  </div>
{/if}
