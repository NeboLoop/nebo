<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { setPluginConfig } from '$lib/api/index';
  import { getWebSocketClient } from '$lib/websocket/client';
  import SetupWizard from '$lib/components/SetupWizard.svelte';

  interface Plugin {
    id: string;
    name: string;
    desc: string;
    author: string;
    version: string;
    hasAuth: boolean;
    authType: string;
    authEnvVars: string[];
    authKeysSet: boolean;
    hasEvents: boolean;
    eventCount: number;
    enabled: boolean;
    updateAvailable: string | null;
    /// When present, this plugin declares a multi-step setup wizard.
    /// The frontend renders SetupWizard.svelte from this config instead
    /// of (or in addition to) the bare token form.
    setup: unknown | null;
  }

  interface Dependent {
    name: string;
    description: string;
    type: 'skill' | 'agent';
  }

  let plugins = $state<Plugin[]>([]);
  let authStatuses = $state<Record<string, 'connected' | 'disconnected' | 'connecting'>>({});
  let selectedPlugin = $state<Plugin | null>(null);
  let modalDependents = $state<Dependent[]>([]);
  let modalLoading = $state(false);
  let removing = $state(false);
  let apiKeyInputs = $state<Record<string, string>>({});
  let apiKeySaving = $state(false);
  let apiKeySaveResult = $state<'saved' | 'error' | null>(null);
  let wizardOpen = $state(false);
  let authChecking = $state(false);

  let unsubscribers: Array<() => void> = [];

  onMount(async () => {
    // Subscribe to WS events immediately — auth status checks below are slow
    // (each spawns a plugin binary) and must not delay event registration.
    const client = getWebSocketClient();
    unsubscribers.push(
      // plugin_auth_url is handled globally in listeners.ts (opens browser)
      client.on('plugin_auth_complete', (data: Record<string, unknown>) => {
        const slug = data.plugin as string;
        if (slug) {
          authStatuses[slug] = 'connected';
        }
      }),
      client.on('plugin_auth_error', (data: Record<string, unknown>) => {
        const slug = data.plugin as string;
        if (slug) {
          authStatuses[slug] = 'disconnected';
        }
      })
    );

    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listPlugins();
      if (resp?.plugins?.length) {
        plugins = resp.plugins.map((p: any) => ({
          id: String(p.id || p.slug || ''),
          name: String(p.name || ''),
          desc: String(p.description || ''),
          author: String(p.author || ''),
          version: String(p.version || ''),
          hasAuth: !!(p.hasAuth ?? p.has_auth ?? false),
          authType: String(p.authType ?? p.auth_type ?? ''),
          authEnvVars: Array.isArray(p.authEnvVars) ? p.authEnvVars.map(String) : [],
          authKeysSet: !!(p.authKeysSet ?? p.auth_keys_set ?? false),
          hasEvents: !!(p.hasEvents ?? false),
          eventCount: Number(p.eventCount ?? 0),
          enabled: p.enabled !== false,
          updateAvailable: p.updateAvailable ?? p.update_available ?? null,
          setup: p.setup ?? null,
        }));

        // Fetch actual auth status for each plugin that has auth
        for (const plugin of plugins) {
          if (!plugin.hasAuth) continue;
          try {
            const status = await api.authStatus(plugin.id);
            authStatuses[plugin.id] = status?.authenticated ? 'connected' : 'disconnected';
          } catch {
            authStatuses[plugin.id] = 'disconnected';
          }
        }
      }
    } catch {}
  });

  onDestroy(() => {
    unsubscribers.forEach((fn) => fn());
  });

  let searchQuery = $state('');

  const filteredPlugins = $derived.by(() => {
    const sorted = [...plugins].sort((a, b) => a.name.localeCompare(b.name));
    if (!searchQuery.trim()) return sorted;
    const q = searchQuery.toLowerCase();
    return sorted.filter(p => p.name.toLowerCase().includes(q) || p.desc.toLowerCase().includes(q) || p.author.toLowerCase().includes(q));
  });

  async function connectPlugin(id: string) {
    authStatuses[id] = 'connecting';
    try {
      const api = await import('$lib/api/nebo');
      await api.authLogin(id);
    } catch {
      authStatuses[id] = 'disconnected';
    }
  }

  async function disconnectPlugin(id: string) {
    authStatuses[id] = 'disconnected';
    try {
      const api = await import('$lib/api/nebo');
      await api.authLogout(id);
    } catch { /* local state already updated */ }
  }

  async function saveApiKeys(plugin: Plugin) {
    if (!plugin.authEnvVars.length) return;
    const payload: Record<string, string> = {};
    for (const key of plugin.authEnvVars) {
      const val = (apiKeyInputs[key] || '').trim();
      if (val) payload[key] = val;
    }
    if (!Object.keys(payload).length) return;
    apiKeySaving = true;
    apiKeySaveResult = null;
    try {
      await setPluginConfig(plugin.id, payload);
      plugin.authKeysSet = true;
      apiKeySaveResult = 'saved';
      apiKeyInputs = {};

      // Run auth check to verify the keys actually work
      authChecking = true;
      try {
        const api = await import('$lib/api/nebo');
        const status = await api.authStatus(plugin.id);
        authStatuses[plugin.id] = status?.authenticated ? 'connected' : 'disconnected';
      } catch {
        authStatuses[plugin.id] = 'disconnected';
      } finally {
        authChecking = false;
      }
    } catch {
      apiKeySaveResult = 'error';
    } finally {
      apiKeySaving = false;
    }
  }

  async function clearApiKeys(plugin: Plugin) {
    if (!plugin.authEnvVars.length) return;
    apiKeySaving = true;
    try {
      const payload: Record<string, string> = {};
      for (const key of plugin.authEnvVars) {
        payload[key] = '';
      }
      await setPluginConfig(plugin.id, payload);
      plugin.authKeysSet = false;
      apiKeyInputs = {};
      authStatuses[plugin.id] = 'disconnected';
    } catch { /* silent */ }
    finally { apiKeySaving = false; }
  }

  async function openPluginDetail(plugin: Plugin) {
    selectedPlugin = plugin;
    modalDependents = [];
    modalLoading = true;
    removing = false;
    apiKeyInputs = {};
    apiKeySaving = false;
    apiKeySaveResult = null;
    authChecking = false;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listDependents(plugin.id);
      const skills = (resp?.skills || []).map((s: any) => ({ ...s, type: 'skill' as const }));
      const agents = (resp?.agents || []).map((a: any) => ({ ...a, type: 'agent' as const }));
      modalDependents = [...skills, ...agents];
    } catch {
      modalDependents = [];
    } finally {
      modalLoading = false;
    }
  }

  function closeModal() {
    selectedPlugin = null;
  }

  async function uninstallPlugin() {
    if (!selectedPlugin || modalDependents.length > 0) return;
    removing = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.removePlugin(selectedPlugin.id);
      plugins = plugins.filter(p => p.id !== selectedPlugin!.id);
      selectedPlugin = null;
    } catch {
      removing = false;
    }
  }

  const canUninstall = $derived(selectedPlugin !== null && modalDependents.length === 0 && !modalLoading);

  // Wizard completes by saving the env vars it collected and then running
  // the verify command (declared on the Credentials step). We reuse the
  // same set-config + auth-status path the manual form uses.
  async function onWizardComplete(envValues: Record<string, string>) {
    if (!selectedPlugin) return;
    await setPluginConfig(selectedPlugin.id, envValues);
    selectedPlugin.authKeysSet = true;
    apiKeySaveResult = 'saved';
    try {
      const api = await import('$lib/api/nebo');
      const status = await api.authStatus(selectedPlugin.id);
      authStatuses[selectedPlugin.id] = status?.authenticated ? 'connected' : 'disconnected';
    } catch {
      authStatuses[selectedPlugin.id] = 'disconnected';
    }
    wizardOpen = false;
  }
</script>

<div class="mb-5">
  <div class="flex items-center justify-between mb-1">
    <h2 class="text-base font-semibold">Plugins</h2>
    <span class="text-xs text-base-content/50 font-mono">{plugins.length} installed</span>
  </div>
  <p class="text-xs text-base-content/70">Manage installed plugins and their connections.</p>
</div>

{#if plugins.length > 0}
  <div class="mb-4">
    <input type="text" bind:value={searchQuery} placeholder="Search plugins…" class="input input-sm input-bordered w-full max-w-xs text-sm" />
  </div>
{/if}

{#if plugins.length === 0}
  <div class="text-center py-12">
    <div class="text-xs text-base-content/50 mb-2">No plugins installed.</div>
    <a href="/marketplace/plugins" class="text-sm text-primary hover:underline">Browse plugins &rarr;</a>
  </div>
{:else if filteredPlugins.length === 0}
  <div class="text-center py-8">
    <div class="text-xs text-base-content/50">No plugins match "{searchQuery}"</div>
  </div>
{:else}
  <div class="flex flex-col gap-2">
    {#each filteredPlugins as plugin}
      <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
        <div class="w-9 h-9 rounded-lg bg-base-200 grid place-items-center text-base shrink-0">&#128268;</div>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <button class="text-sm font-medium text-primary hover:underline cursor-pointer bg-transparent border-none p-0 text-left" onclick={() => openPluginDetail(plugin)}>{plugin.name}</button>
            {#if plugin.version}
              <span class="text-xs text-base-content/50 font-mono">{plugin.version}</span>
            {/if}
          </div>
          {#if plugin.desc}
            <div class="text-xs text-base-content/70 mt-0.5 line-clamp-1">{plugin.desc}</div>
          {/if}
          {#if plugin.author || plugin.hasEvents}
            <div class="flex items-center gap-2 mt-1">
              {#if plugin.author}
                <span class="text-xs text-base-content/50">by {plugin.author}</span>
              {/if}
              {#if plugin.author && plugin.hasEvents}
                <span class="text-xs text-base-content/30">&middot;</span>
              {/if}
              {#if plugin.hasEvents}
                <span class="text-xs text-base-content/50">{plugin.eventCount} {plugin.eventCount === 1 ? 'event' : 'events'}</span>
              {/if}
            </div>
          {/if}
        </div>
        <div class="flex items-center gap-2 shrink-0">
          {#if plugin.hasAuth && plugin.authEnvVars.length > 0 && !plugin.authKeysSet}
            <button class="px-3 py-1 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors" onclick={() => openPluginDetail(plugin)}>Set API Keys</button>
          {:else if plugin.hasAuth && plugin.authType !== 'env'}
            {@const status = authStatuses[plugin.id] ?? 'disconnected'}
            {#if status === 'connected'}
              <span class="px-2 py-0.5 rounded text-xs font-medium bg-success/10 text-success">Connected</span>
              <button class="px-3 py-1 rounded-md border border-base-content/10 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={() => disconnectPlugin(plugin.id)}>Disconnect</button>
            {:else if status === 'connecting'}
              <span class="px-2 py-0.5 rounded text-xs font-medium bg-info/10 text-info">Connecting…</span>
            {:else}
              <button class="px-3 py-1 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors" onclick={() => connectPlugin(plugin.id)}>Connect</button>
            {/if}
          {:else if plugin.hasAuth && plugin.authType === 'env'}
            {#if plugin.authKeysSet}
              <span class="px-2 py-0.5 rounded text-xs font-medium bg-success/10 text-success">Key Set</span>
            {/if}
            <button class="px-3 py-1 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors" onclick={() => openPluginDetail(plugin)}>{plugin.authKeysSet ? 'Update Keys' : 'Set API Keys'}</button>
          {:else}
            <span class="text-xs text-base-content/40">No auth needed</span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<!-- Plugin Detail Modal -->
{#if selectedPlugin}
  {@const status = authStatuses[selectedPlugin.id] ?? 'disconnected'}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_interactive_supports_focus a11y_no_noninteractive_tabindex -->
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40" tabindex="-1" onclick={(e) => { if (e.target === e.currentTarget) closeModal(); }} onkeydown={(e) => { if (e.key === 'Escape') closeModal(); }} role="dialog" aria-modal="true">
    <div class="bg-base-100 rounded-xl border border-base-300 shadow-xl w-full max-w-xl mx-4 max-h-[80vh] flex flex-col">
      <!-- Header -->
      <div class="flex items-center justify-between p-5 border-b border-base-content/10">
        <div class="flex items-center gap-3 min-w-0">
          <div class="w-10 h-10 rounded-lg bg-base-200 grid place-items-center text-lg shrink-0">&#128268;</div>
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-base font-semibold">{selectedPlugin.name}</span>
              {#if selectedPlugin.version}
                <span class="text-xs text-base-content/50 font-mono">{selectedPlugin.version}</span>
              {/if}
            </div>
            {#if selectedPlugin.author}
              <div class="text-xs text-base-content/50">by {selectedPlugin.author}</div>
            {/if}
          </div>
        </div>
        <button class="btn btn-ghost btn-sm btn-square" onclick={closeModal} aria-label="Close">
          <svg xmlns="http://www.w3.org/2000/svg" class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
        </button>
      </div>

      <!-- Body -->
      <div class="p-5 overflow-y-auto flex-1 space-y-6">
        <!-- Description -->
        {#if selectedPlugin.desc}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Description</div>
            <p class="text-xs text-base-content/70 line-clamp-3">{selectedPlugin.desc}</p>
          </div>
        {/if}

        <!-- Status -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Status</div>
          <div class="flex items-center gap-3">
            {#if selectedPlugin.hasAuth && selectedPlugin.authType !== 'env'}
              {#if !selectedPlugin.authKeysSet && selectedPlugin.authEnvVars.length > 0}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">Credentials needed</span>
              {:else if status === 'connected'}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-success/10 text-success">Connected</span>
              {:else if status === 'connecting'}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-info/10 text-info">Connecting…</span>
              {:else}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">Not connected</span>
              {/if}
            {:else if selectedPlugin.hasAuth && selectedPlugin.authEnvVars.length > 0 && selectedPlugin.authType === 'env'}
              {#if selectedPlugin.authKeysSet && authStatuses[selectedPlugin.id] === 'connected'}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-success/10 text-success">Connected</span>
              {:else if selectedPlugin.authKeysSet}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">Keys Set &middot; Not verified</span>
              {:else}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">Keys needed</span>
              {/if}
            {:else}
              <span class="text-xs text-base-content/50">No authentication required</span>
            {/if}
            {#if selectedPlugin.hasEvents}
              <span class="text-xs text-base-content/50">{selectedPlugin.eventCount} {selectedPlugin.eventCount === 1 ? 'event' : 'events'}</span>
            {/if}
          </div>
        </div>

        <!-- Setup wizard launcher (only when the plugin declares one) -->
        {#if selectedPlugin.setup}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Guided Setup</div>
            <button class="btn btn-sm btn-primary" onclick={() => { wizardOpen = true; }}>
              {selectedPlugin.authKeysSet ? 'Reconfigure' : 'Run setup wizard'}
            </button>
            <p class="text-xs text-base-content/70 mt-2">
              Walks through manifest generation, app install, and credentials in one flow.
            </p>
          </div>
        {/if}

        <!-- API Keys / Credentials -->
        {#if selectedPlugin.hasAuth && selectedPlugin.authEnvVars.length > 0}
          {@const hasInput = Object.values(apiKeyInputs).some(v => v.trim())}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">API Keys</div>
            <div class="flex flex-col gap-3">
              {#each selectedPlugin.authEnvVars as envVar}
                <label class="flex flex-col gap-1">
                  <span class="text-xs text-base-content/70 font-mono">{envVar}</span>
                  <input type="password" value={apiKeyInputs[envVar] ?? ''} oninput={(e) => { apiKeySaveResult = null; apiKeyInputs[envVar] = (e.target as HTMLInputElement).value; }} placeholder={selectedPlugin.authKeysSet ? '••••••••' : 'Paste token here…'} class="input input-sm input-bordered w-full text-sm font-mono"
                    onkeydown={(e) => { if (e.key === 'Enter' && selectedPlugin) saveApiKeys(selectedPlugin); }}
                  />
                </label>
              {/each}
            </div>
            <div class="flex items-center gap-3 mt-3">
              <button
                class="btn btn-sm btn-primary"
                disabled={!hasInput || apiKeySaving}
                onclick={() => selectedPlugin && saveApiKeys(selectedPlugin)}
              >{apiKeySaving ? 'Saving…' : authChecking ? 'Verifying…' : 'Save & Verify'}</button>
              {#if apiKeySaveResult === 'saved' && !authChecking}
                {@const authed = authStatuses[selectedPlugin.id] === 'connected'}
                {#if authed}
                  <span class="text-xs font-medium text-success">Authenticated</span>
                {:else}
                  <span class="text-xs font-medium text-error">Keys saved but authentication failed — check your tokens</span>
                {/if}
              {:else if apiKeySaveResult === 'error'}
                <span class="text-xs font-medium text-error">Failed to save</span>
              {/if}
            </div>
          </div>
        {/if}

        <!-- Dependents -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Used by</div>
          {#if modalLoading}
            <div class="text-xs text-base-content/50">Loading…</div>
          {:else if modalDependents.length === 0}
            <div class="text-xs text-base-content/50">No skills or agents depend on this plugin.</div>
          {:else}
            <div class="flex flex-col gap-1.5">
              {#each modalDependents as dep}
                <div class="flex items-start gap-2.5 px-3 py-2.5 rounded-lg bg-base-200/50 border border-base-content/5">
                  <span class="px-1.5 py-0.5 rounded text-[10px] font-medium uppercase tracking-wider shrink-0 mt-0.5 {dep.type === 'agent' ? 'bg-primary/10 text-primary' : 'bg-accent/10 text-accent'}">{dep.type}</span>
                  <div class="min-w-0">
                    <div class="text-sm font-medium">{dep.name}</div>
                    {#if dep.description}
                      <div class="text-xs text-base-content/50 truncate">{dep.description}</div>
                    {/if}
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>

      <!-- Footer Actions -->
      <div class="flex items-center justify-between p-5 border-t border-base-content/10">
        <div class="flex items-center gap-2">
          {#if selectedPlugin.hasAuth && selectedPlugin.authType !== 'env'}
            {#if status === 'connected'}
              <button class="px-3 py-1.5 rounded-md border border-base-content/10 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={() => disconnectPlugin(selectedPlugin!.id)}>Disconnect</button>
            {:else if status !== 'connecting'}
              <button class="px-3 py-1.5 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors" onclick={() => connectPlugin(selectedPlugin!.id)}>Connect</button>
            {/if}
          {:else if selectedPlugin.hasAuth && selectedPlugin.authType === 'env' && selectedPlugin.authKeysSet}
            <button class="px-3 py-1.5 rounded-md border border-base-content/10 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors" disabled={apiKeySaving} onclick={() => clearApiKeys(selectedPlugin!)}>Clear Keys</button>
          {/if}
          {#if selectedPlugin.updateAvailable}
            <a href="/marketplace/plugins/{selectedPlugin.id}" class="px-3 py-1.5 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors no-underline">Upgrade to {selectedPlugin.updateAvailable}</a>
          {/if}
        </div>
        <div>
          {#if canUninstall}
            <button class="px-3 py-1.5 rounded-md border border-error/30 text-xs text-error font-medium cursor-pointer bg-transparent hover:bg-error/5 transition-colors" onclick={uninstallPlugin} disabled={removing}>
              {removing ? 'Removing…' : 'Uninstall'}
            </button>
          {:else if !modalLoading && modalDependents.length > 0}
            <div class="tooltip tooltip-left" data-tip="Cannot uninstall — {modalDependents.length} {modalDependents.length === 1 ? 'item depends' : 'items depend'} on this plugin">
              <button class="px-3 py-1.5 rounded-md border border-base-content/10 text-xs text-base-content/30 cursor-not-allowed bg-transparent" disabled>Uninstall</button>
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>
{/if}

{#if wizardOpen && selectedPlugin?.setup}
  <SetupWizard
    slug={selectedPlugin.id}
    setup={selectedPlugin.setup as any}
    onClose={() => { wizardOpen = false; }}
    onComplete={onWizardComplete}
  />
{/if}
