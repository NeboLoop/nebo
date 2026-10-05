<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { setPluginConfig } from '$lib/api/index';
  import { getWebSocketClient } from '$lib/websocket/client';
  import SetupWizard from '$lib/components/SetupWizard.svelte';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import StatCard from '$lib/components/settings/StatCard.svelte';
  import BrowseCard from '$lib/components/settings/BrowseCard.svelte';
  import ConfirmModal from '$lib/components/settings/ConfirmModal.svelte';

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
    /// Accounts live per employee; no plugin-level keys.
    multiAccount: boolean;
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
  let confirmingUninstall = $state(false);
  let apiKeyInputs = $state<Record<string, string>>({});
  let apiKeySaving = $state(false);
  let apiKeySaveResult = $state<'saved' | 'error' | null>(null);
  let wizardOpen = $state(false);
  let authChecking = $state(false);
  let confirmingDisconnect = $state(false);

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
      }),
      // An update applied (or failed) — from this page, the product page, or
      // the Updates page: the row's version and badge come from the reload.
      client.on('artifact_update_applied', (data: Record<string, unknown>) => {
        const id = String(data.id ?? '');
        if (id) updating = { ...updating, [id]: false };
        loadPlugins();
      }),
      client.on('artifact_update_failed', (data: Record<string, unknown>) => {
        const id = String(data.id ?? '');
        if (id) updating = { ...updating, [id]: false };
      })
    );

    await loadPlugins();
  });

  // Plugin ids mid-update (button → "Updating…"); cleared by the WS result.
  let updating = $state<Record<string, boolean>>({});

  async function updatePlugin(plugin: Plugin) {
    updating = { ...updating, [plugin.id]: true };
    try {
      const api = await import('$lib/api/nebo');
      // The pending-update row for a plugin is keyed by its slug — the same
      // id the Updates page and the product page apply with.
      await api.applyUpdate(plugin.id);
    } catch {
      updating = { ...updating, [plugin.id]: false };
    }
  }

  async function loadPlugins() {
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
          // Accounts live per employee (Settings → Accounts on the employee);
          // there are no plugin-level keys to set.
          multiAccount: !!(p.multiAccount ?? false),
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
  }

  onDestroy(() => {
    unsubscribers.forEach((fn) => fn());
  });

  let searchQuery = $state('');

  const connectedCount = $derived(plugins.filter((p) => authStatuses[p.id] === 'connected').length);

  /** A row's status: Connected, Ready (nothing to sign in to), Connecting,
   *  Not connected, or — for a plugin whose accounts live on each employee —
   *  that. Connect and Disconnect live in the plugin's detail. */
  type RowStatus = 'connected' | 'ready' | 'connecting' | 'notConnected' | 'perEmployee';
  function rowStatus(plugin: Plugin): RowStatus {
    if (!plugin.hasAuth) return 'ready';
    if (plugin.multiAccount) return 'perEmployee';
    const status = authStatuses[plugin.id];
    if (status === 'connected') return 'connected';
    if (status === 'connecting') return 'connecting';
    return 'notConnected';
  }

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
    confirmingUninstall = false;
    confirmingDisconnect = false;
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

<SettingsHeader title={$t('settingsPlugins.title')} description={$t('settingsPlugins.pageDescription')} />

<div class="flex gap-3 mb-6">
  <StatCard label={$t('common.installed')} value={plugins.length} />
  <StatCard label={$t('common.connected')} value={connectedCount} accent="success" />
</div>

<div class="mb-6">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-base font-semibold">{$t('settingsPlugins.installedPlugins')}</h3>
    {#if plugins.length > 0}
      <input type="text" bind:value={searchQuery} placeholder={$t('settingsPlugins.searchPlaceholder')} class="input input-sm input-bordered max-w-xs text-sm" />
    {/if}
  </div>

  {#if plugins.length === 0}
    <div class="text-center py-12">
      <div class="text-xs text-base-content/50 mb-2">{$t('settingsPlugins.noneInstalled')}</div>
      <a href="/marketplace/plugins" class="text-sm text-primary hover:underline">{$t('settingsPlugins.browsePluginsArrow')}</a>
    </div>
  {:else if filteredPlugins.length === 0}
    <div class="text-center py-8">
      <div class="text-xs text-base-content/50">{$t('settingsPlugins.noMatch', { values: { search: searchQuery } })}</div>
    </div>
  {:else}
    <div class="flex flex-col gap-1.5">
      {#each filteredPlugins as plugin (plugin.id)}
        {@const status = rowStatus(plugin)}
        <!-- The whole row opens the plugin's detail; Connect and Disconnect live there. -->
        <div
          role="button"
          tabindex="0"
          class="flex items-center gap-3 p-3.5 rounded-lg border border-base-300 bg-base-100 hover:border-base-content/20 hover:bg-base-200/40 transition-colors cursor-pointer"
          onclick={() => openPluginDetail(plugin)}
          onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); openPluginDetail(plugin); } }}
        >
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2 min-w-0">
              <span class="text-sm font-medium truncate">{plugin.name}</span>
              {#if plugin.version}
                <span class="text-xs text-base-content/50 font-mono shrink-0">{plugin.version}</span>
              {/if}
              {#if plugin.updateAvailable}
                <button type="button" class="py-0.5 px-2 rounded bg-primary/15 text-primary text-xs font-medium border-none cursor-pointer hover:bg-primary/25 transition-colors disabled:opacity-60 disabled:cursor-default shrink-0" disabled={updating[plugin.id]} onclick={(e) => { e.stopPropagation(); updatePlugin(plugin); }} onkeydown={(e) => e.stopPropagation()}>{updating[plugin.id] ? $t('agentSettings.updating') : $t('agentSettings.updateTo', { values: { version: plugin.updateAvailable } })}</button>
              {/if}
            </div>
            {#if plugin.desc}
              <div class="text-xs text-base-content/70 mt-0.5 line-clamp-2">{plugin.desc}</div>
            {/if}
            {#if plugin.author}
              <div class="text-xs text-base-content/50 mt-1">{$t('settingsApps.byAuthor', { values: { name: plugin.author } })}</div>
            {/if}
          </div>
          <span class="flex items-center gap-1.5 shrink-0 text-xs {status === 'connected' || status === 'ready' ? 'text-base-content/70' : 'text-base-content/50'}">
            {#if status === 'connecting'}
              <span class="loading loading-spinner loading-xs text-info"></span>
            {:else}
              <span class="w-2 h-2 rounded-full {status === 'connected' || status === 'ready' ? 'bg-success' : 'bg-base-content/20'}"></span>
            {/if}
            {#if status === 'connected'}{$t('settingsPlugins.connected')}
            {:else if status === 'ready'}{$t('settingsPlugins.ready')}
            {:else if status === 'connecting'}{$t('settingsPlugins.connecting')}
            {:else if status === 'perEmployee'}{$t('settingsPlugins.perEmployeeAccounts')}
            {:else}{$t('settingsPlugins.notConnected')}{/if}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<BrowseCard title={$t('settingsPlugins.browseTitle')} description={$t('settingsPlugins.browseDescription')} href="/marketplace/plugins" />

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
              <div class="text-xs text-base-content/50">{$t('settingsApps.byAuthor', { values: { name: selectedPlugin.author } })}</div>
            {/if}
          </div>
        </div>
        <button class="btn btn-ghost btn-sm btn-square" onclick={closeModal} aria-label={$t('common.close')}>
          <svg xmlns="http://www.w3.org/2000/svg" class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
        </button>
      </div>

      <!-- Body -->
      <div class="p-5 overflow-y-auto flex-1 space-y-6">
        <!-- Description -->
        {#if selectedPlugin.desc}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">{$t('common.description')}</div>
            <p class="text-xs text-base-content/70 line-clamp-3">{selectedPlugin.desc}</p>
          </div>
        {/if}

        <!-- Status -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('common.status')}</div>
          <div class="flex items-center gap-3">
            {#if selectedPlugin.hasAuth && selectedPlugin.authType !== 'env'}
              {#if !selectedPlugin.authKeysSet && selectedPlugin.authEnvVars.length > 0}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">{$t('settingsPlugins.credentialsNeeded')}</span>
              {:else if status === 'connected'}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-success/10 text-success">{$t('settingsPlugins.connected')}</span>
              {:else if status === 'connecting'}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-info/10 text-info">{$t('settingsPlugins.connecting')}</span>
              {:else}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">{$t('settingsProviders.notConnected')}</span>
              {/if}
            {:else if selectedPlugin.hasAuth && selectedPlugin.authEnvVars.length > 0 && selectedPlugin.authType === 'env' && !selectedPlugin.multiAccount}
              {#if selectedPlugin.authKeysSet && authStatuses[selectedPlugin.id] === 'connected'}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-success/10 text-success">{$t('settingsPlugins.connected')}</span>
              {:else if selectedPlugin.authKeysSet}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">{$t('settingsPlugins.keysSetNotVerified')}</span>
              {:else}
                <span class="px-2 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">{$t('settingsPlugins.keysNeeded')}</span>
              {/if}
            {:else}
              <span class="text-xs text-base-content/50">{$t('settingsMcp.authNoneDesc')}</span>
            {/if}
            {#if selectedPlugin.hasEvents}
              <span class="text-xs text-base-content/50">{selectedPlugin.eventCount === 1 ? $t('settingsPlugins.eventCountSingular', { values: { count: selectedPlugin.eventCount } }) : $t('settingsPlugins.eventCount', { values: { count: selectedPlugin.eventCount } })}</span>
            {/if}
          </div>
        </div>

        <!-- Setup wizard launcher (only when the plugin declares one) -->
        {#if selectedPlugin.setup}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('settingsPlugins.guidedSetup')}</div>
            <button class="btn btn-sm btn-primary" onclick={() => { wizardOpen = true; }}>
              {selectedPlugin.authKeysSet ? $t('settingsPlugins.reconfigure') : $t('settingsPlugins.runSetupWizard')}
            </button>
            <p class="text-xs text-base-content/70 mt-2">
              {$t('settingsPlugins.wizardDesc')}
            </p>
          </div>
        {/if}

        <!-- API Keys / Credentials -->
        {#if selectedPlugin.hasAuth && selectedPlugin.authEnvVars.length > 0 && !selectedPlugin.multiAccount}
          {@const hasInput = Object.values(apiKeyInputs).some(v => v.trim())}
          <div>
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('settingsProviders.apiKeys')}</div>
            <div class="flex flex-col gap-3">
              {#each selectedPlugin.authEnvVars as envVar}
                <label class="flex flex-col gap-1">
                  <span class="text-xs text-base-content/70 font-mono">{envVar}</span>
                  <input type="password" value={apiKeyInputs[envVar] ?? ''} oninput={(e) => { apiKeySaveResult = null; apiKeyInputs[envVar] = (e.target as HTMLInputElement).value; }} placeholder={selectedPlugin.authKeysSet ? '••••••••' : $t('settingsPlugins.pasteTokenPlaceholder')} class="input input-sm input-bordered w-full text-sm font-mono"
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
              >{apiKeySaving ? $t('common.saving') : authChecking ? $t('onboarding.apiKey.verifying') : $t('settingsPlugins.saveAndVerify')}</button>
              {#if apiKeySaveResult === 'saved' && !authChecking}
                {@const authed = authStatuses[selectedPlugin.id] === 'connected'}
                {#if authed}
                  <span class="text-xs font-medium text-success">{$t('settingsPlugins.authenticated')}</span>
                {:else}
                  <span class="text-xs font-medium text-error">{$t('settingsPlugins.keysSavedAuthFailed')}</span>
                {/if}
              {:else if apiKeySaveResult === 'error'}
                <span class="text-xs font-medium text-error">{$t('settingsIdentity.saveFailed')}</span>
              {/if}
            </div>
          </div>
        {/if}

        <!-- Dependents -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">{$t('settingsPlugins.usedBy')}</div>
          {#if modalLoading}
            <div class="text-xs text-base-content/50">{$t('common.loading')}</div>
          {:else if modalDependents.length === 0}
            <div class="text-xs text-base-content/50">{$t('settingsPlugins.noDependents')}</div>
          {:else}
            <div class="flex flex-col gap-1.5">
              {#each modalDependents as dep}
                <div class="flex items-start gap-2.5 px-3 py-2.5 rounded-lg bg-base-200/50 border border-base-content/5">
                  <span class="px-1.5 py-0.5 rounded text-[0.625rem] font-medium uppercase tracking-wider shrink-0 mt-0.5 {dep.type === 'agent' ? 'bg-primary/10 text-primary' : 'bg-accent/10 text-accent'}">{dep.type}</span>
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
              <button type="button" class="btn btn-ghost btn-sm text-error hover:bg-error/10" onclick={() => (confirmingDisconnect = true)}>{$t('settingsPlugins.disconnect')}</button>
            {:else if status !== 'connecting'}
              <button class="px-3 py-1.5 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors" onclick={() => connectPlugin(selectedPlugin!.id)}>{$t('settingsPlugins.connect')}</button>
            {/if}
          {:else if selectedPlugin.hasAuth && selectedPlugin.authType === 'env' && selectedPlugin.authKeysSet}
            <button class="px-3 py-1.5 rounded-md border border-base-content/10 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors" disabled={apiKeySaving} onclick={() => clearApiKeys(selectedPlugin!)}>{$t('settingsPlugins.clearKeys')}</button>
          {/if}
          {#if selectedPlugin.updateAvailable}
            <button type="button" class="px-3 py-1.5 rounded-md border border-primary/30 text-xs text-primary font-medium cursor-pointer bg-transparent hover:bg-primary/5 transition-colors disabled:opacity-60 disabled:cursor-default" disabled={updating[selectedPlugin.id]} onclick={() => selectedPlugin && updatePlugin(selectedPlugin)}>{updating[selectedPlugin.id] ? $t('agentSettings.updating') : $t('settingsPlugins.upgradeTo', { values: { version: selectedPlugin.updateAvailable } })}</button>
          {/if}
        </div>
        <div>
          {#if canUninstall}
            <button class="px-3 py-1.5 rounded-md border border-error/30 text-xs text-error font-medium cursor-pointer bg-transparent hover:bg-error/5 transition-colors" onclick={() => (confirmingUninstall = true)}>
              {$t('common.uninstall')}
            </button>
          {:else if !modalLoading && modalDependents.length > 0}
            <div class="tooltip tooltip-left" data-tip={modalDependents.length === 1 ? $t('settingsPlugins.cannotUninstallSingular', { values: { count: modalDependents.length } }) : $t('settingsPlugins.cannotUninstall', { values: { count: modalDependents.length } })}>
              <button class="px-3 py-1.5 rounded-md border border-base-content/10 text-xs text-base-content/30 cursor-not-allowed bg-transparent" disabled>{$t('common.uninstall')}</button>
            </div>
          {/if}
        </div>
      </div>
    </div>
  </div>

  {#if confirmingDisconnect && selectedPlugin}
    <ConfirmModal
      title={$t('settingsPlugins.disconnectTitle', { values: { name: selectedPlugin.name } })}
      message={$t('settingsPlugins.disconnectMessage', { values: { name: selectedPlugin.name } })}
      confirmLabel={$t('settingsPlugins.disconnect')}
      onCancel={() => (confirmingDisconnect = false)}
      onConfirm={() => { confirmingDisconnect = false; if (selectedPlugin) void disconnectPlugin(selectedPlugin.id); }}
    />
  {/if}

  {#if confirmingUninstall && selectedPlugin}
    <ConfirmModal
      title={$t('common.uninstallTitle', { values: { name: selectedPlugin.name } })}
      message={$t('settingsPlugins.uninstallMessage')}
      confirmLabel={$t('common.uninstall')}
      busy={removing}
      onCancel={() => (confirmingUninstall = false)}
      onConfirm={uninstallPlugin}
    />
  {/if}
{/if}

{#if wizardOpen && selectedPlugin?.setup}
  <SetupWizard
    slug={selectedPlugin.id}
    setup={selectedPlugin.setup as any}
    onClose={() => { wizardOpen = false; }}
    onComplete={onWizardComplete}
  />
{/if}
