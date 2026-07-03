<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount, onDestroy } from 'svelte';
  import Power from 'lucide-svelte/icons/power';
  import Plus from 'lucide-svelte/icons/plus';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import X from 'lucide-svelte/icons/x';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import KeyRound from 'lucide-svelte/icons/key-round';
  import type { McpIntegration } from '$lib/api/nebo';

  interface MCPIntegration { id: string; name: string; serverUrl: string; authType: 'oauth' | 'api_key' | 'none'; isEnabled: boolean; connectionStatus: 'connected' | 'disconnected' | 'error'; toolCount: number; lastConnectedAt: string; lastError: string | null }

  let integrations = $state<MCPIntegration[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const intResp = await api.listIntegrations();
      if (intResp?.integrations?.length) {
        integrations = intResp.integrations.map((i: McpIntegration) => ({
          id: i.id,
          name: i.name,
          serverUrl: i.serverUrl || '',
          authType: (i.authType || 'oauth') as 'oauth' | 'api_key' | 'none',
          isEnabled: i.isEnabled ?? false,
          connectionStatus: (i.connectionStatus || 'disconnected') as 'connected' | 'disconnected' | 'error',
          toolCount: i.toolCount ?? 0,
          lastConnectedAt: i.lastConnectedAt ? new Date(i.lastConnectedAt * 1000).toLocaleString() : 'Never',
          lastError: i.lastError || null,
        }));
      }
    } catch { /* leave list empty on error */ }
  });
  // "Add MCP Server" is for adding ONE custom server (URL + auth). Discovery of
  // published MCP servers lives in the Marketplace (connectors), not here — we
  // expect thousands, so this is never a catalog/search surface.
  let showAddModal = $state(false);
  let addStep = $state<'auth' | 'configure'>('auth');
  let newServerUrl = $state('');
  let newServerName = $state('');
  let newApiKey = $state('');
  let newAuthType = $state<'oauth' | 'api_key' | 'none'>('oauth');

  const currentStep = $derived(addStep === 'auth' ? 1 : 2);
  const totalSteps = 2;

  function openAddModal() {
    showAddModal = true;
    addStep = 'auth';
    newServerUrl = '';
    newServerName = '';
    newApiKey = '';
    newAuthType = 'oauth';
  }

  function closeAddModal() {
    showAddModal = false;
  }

  function goBack() {
    if (addStep === 'configure') {
      addStep = 'auth';
    } else {
      closeAddModal();
    }
  }

  function goNext() {
    if (addStep === 'auth') {
      addStep = 'configure';
    }
  }

  const authOptions = [
    { value: 'oauth' as const, label: 'OAuth 2.1', description: 'Recommended — secure token-based auth' },
    { value: 'api_key' as const, label: 'API Key / Bearer Token', description: 'Authenticate with a static token' },
    { value: 'none' as const, label: 'None', description: 'No authentication required' },
  ];

  let oauthPollingId: ReturnType<typeof setInterval> | null = null;

  onDestroy(() => {
    if (oauthPollingId) clearInterval(oauthPollingId);
  });

  function updateIntegrationById(id: string, patch: Partial<MCPIntegration>) {
    integrations = integrations.map(i => i.id === id ? { ...i, ...patch } : i);
  }

  /** Open auth URL and poll for OAuth completion. If authUrl is provided, use it directly; otherwise fetch via getOauthUrl. */
  async function startOAuthFlow(id: string, authUrl?: string) {
    // Prevent double-open if already polling
    if (oauthPollingId) return;
    const api = await import('$lib/api/nebo');
    if (!authUrl) {
      const oauthResp = await api.getOauthUrl(id) as { authUrl?: string };
      authUrl = oauthResp?.authUrl;
    }
    if (!authUrl) return;
    window.open(authUrl, '_blank');
    updateIntegrationById(id, { connectionStatus: 'disconnected', lastError: 'Waiting for OAuth authorization...' });
    // Poll for OAuth completion — the callback stores tokens and the connect call succeeds
    oauthPollingId = setInterval(async () => {
      try {
        const resp = await api.connectIntegration(id);
        const result = resp as { success?: boolean; toolCount?: number; message?: string };
        if (result?.success) {
          if (oauthPollingId) { clearInterval(oauthPollingId); oauthPollingId = null; }
          updateIntegrationById(id, {
            isEnabled: true,
            connectionStatus: 'connected',
            toolCount: result.toolCount ?? 0,
            lastError: null,
          });
        }
      } catch {
        // OAuth not complete yet — keep polling
      }
    }, 3000);
    // Stop polling after 3 minutes
    setTimeout(() => {
      if (oauthPollingId) {
        clearInterval(oauthPollingId);
        oauthPollingId = null;
        const item = integrations.find(i => i.id === id);
        if (item?.connectionStatus !== 'connected') {
          updateIntegrationById(id, { lastError: 'OAuth timed out. Try reconnecting.' });
        }
      }
    }, 180_000);
  }

  async function addIntegration() {
    const name = newServerName;
    if (!name.trim() || !newServerUrl.trim()) return;
    const newItem: MCPIntegration = {
      id: `int_${Date.now()}`,
      name,
      serverUrl: newServerUrl,
      authType: newAuthType as 'oauth' | 'api_key' | 'none',
      isEnabled: false,
      connectionStatus: 'disconnected',
      toolCount: 0,
      lastConnectedAt: 'Never',
      lastError: null,
    };
    integrations = [...integrations, newItem];
    closeAddModal();
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.createIntegration({
        name,
        serverUrl: newServerUrl,
        authType: newAuthType,
        apiKey: newAuthType === 'api_key' ? newApiKey : undefined,
      });
      if (resp?.integration?.id) {
        const realId = resp.integration.id;
        updateIntegrationById(newItem.id, { id: realId });
        // For OAuth integrations, start the OAuth flow immediately
        if (newAuthType === 'oauth') {
          await startOAuthFlow(realId);
        } else {
          // For non-OAuth, connect directly
          await api.connectIntegration(realId);
          updateIntegrationById(realId, { isEnabled: true, connectionStatus: 'connected' });
        }
      }
    } catch { /* local state already has the item */ }
  }

  async function toggleEnabled(id: string) {
    const item = integrations.find(i => i.id === id);
    if (!item) return;

    if (item.isEnabled) {
      // Disconnecting
      updateIntegrationById(id, { isEnabled: false, connectionStatus: 'disconnected', lastError: null });
      try {
        const api = await import('$lib/api/nebo');
        await api.updateIntegration(id, { isEnabled: false });
      } catch { /* local state already updated */ }
    } else {
      // Connecting
      updateIntegrationById(id, { lastError: null });
      try {
        const api = await import('$lib/api/nebo');
        if (item.authType === 'oauth') {
          // Try connecting first (tokens may already exist from previous OAuth)
          const resp = await api.connectIntegration(id) as { success?: boolean; toolCount?: number };
          if (resp?.success) {
            updateIntegrationById(id, { isEnabled: true, connectionStatus: 'connected', toolCount: resp.toolCount ?? 0 });
          } else {
            // No tokens — start OAuth flow
            await startOAuthFlow(id);
          }
        } else {
          await api.connectIntegration(id);
          updateIntegrationById(id, { isEnabled: true, connectionStatus: 'connected' });
        }
      } catch {
        // Connect failed — for OAuth, try starting the flow
        if (item.authType === 'oauth') {
          try { await startOAuthFlow(id); } catch { updateIntegrationById(id, { lastError: 'Failed to start OAuth' }); }
        } else {
          updateIntegrationById(id, { connectionStatus: 'error', lastError: 'Connection failed' });
        }
      }
    }
  }

  async function testConnection(id: string) {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.testIntegration(id) as { success?: boolean; message?: string };
      if (resp?.success) {
        updateIntegrationById(id, { lastError: null });
      } else {
        updateIntegrationById(id, { lastError: resp?.message || 'Test failed' });
      }
    } catch {
      updateIntegrationById(id, { lastError: 'Test request failed' });
    }
  }

  // API-key entry for existing integrations (connector installs arrive without a key).
  let keyPromptId = $state<string | null>(null);
  let keyPromptValue = $state('');

  function openKeyPrompt(id: string) {
    keyPromptId = id;
    keyPromptValue = '';
  }

  async function submitApiKey() {
    const id = keyPromptId;
    const apiKey = keyPromptValue.trim();
    if (!id || !apiKey) return;
    keyPromptId = null;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateIntegration(id, { apiKey, isEnabled: true });
      const resp = await api.connectIntegration(id) as { success?: boolean; toolCount?: number; message?: string };
      if (resp?.success) {
        updateIntegrationById(id, { isEnabled: true, connectionStatus: 'connected', toolCount: resp.toolCount ?? 0, lastError: null });
      } else {
        updateIntegrationById(id, { connectionStatus: 'error', lastError: resp?.message || 'Connection failed — check the key' });
      }
    } catch {
      updateIntegrationById(id, { connectionStatus: 'error', lastError: 'Failed to save API key' });
    }
  }

  async function reauthenticate(id: string) {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.reauthenticateIntegration(id) as { authUrl?: string };
      if (resp?.authUrl) {
        await startOAuthFlow(id, resp.authUrl);
      }
    } catch {
      updateIntegrationById(id, { lastError: 'Reauthentication failed' });
    }
  }

  async function removeIntegration(id: string) {
    integrations = integrations.filter(i => i.id !== id);
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteIntegration(id);
    } catch { /* local state already updated */ }
  }

  const connectedCount = $derived(integrations.filter(i => i.connectionStatus === 'connected').length);
  const totalTools = $derived(integrations.filter(i => i.isEnabled).reduce((sum, i) => sum + i.toolCount, 0));

  const configureDisabled = $derived(() => {
    if (!newServerUrl.trim()) return true;
    if (!newServerName.trim()) return true;
    if (newAuthType === 'api_key' && !newApiKey.trim()) return true;
    return false;
  });

  const submitLabel = $derived(
    newAuthType === 'oauth' ? 'Connect with OAuth' : 'Add Server'
  );
</script>

<SettingsHeader title="MCP Servers" description="Manage Model Context Protocol server connections." />

<!-- Summary -->
<div class="flex gap-3 mb-6">
  <div class="flex-1 p-3.5 rounded-lg border border-base-300 bg-base-100">
    <div class="text-xs text-base-content/50 mb-0.5">Servers</div>
    <div class="text-lg font-bold">{integrations.length}</div>
  </div>
  <div class="flex-1 p-3.5 rounded-lg border border-base-300 bg-base-100">
    <div class="text-xs text-base-content/50 mb-0.5">Connected</div>
    <div class="text-lg font-bold text-success">{connectedCount}</div>
  </div>
  <div class="flex-1 p-3.5 rounded-lg border border-base-300 bg-base-100">
    <div class="text-xs text-base-content/50 mb-0.5">Total Tools</div>
    <div class="text-lg font-bold">{totalTools}</div>
  </div>
</div>

<!-- Server list -->
<div class="mb-6">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-base font-semibold">Configured Servers</h3>
    <button
      onclick={openAddModal}
      class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-base-300 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
    >
      <Plus class="w-3.5 h-3.5" /> Add server
    </button>
  </div>

  <div class="flex flex-col gap-1.5">
    {#each integrations as integration}
      <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-300 bg-base-100">
        <div class="w-2 h-2 rounded-full shrink-0 {integration.connectionStatus === 'connected' ? 'bg-success' : integration.lastError ? 'bg-error' : 'bg-base-content/20'}" title={integration.connectionStatus === 'connected' ? 'Connected' : integration.lastError ?? 'Disconnected'}></div>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2 mb-0.5">
            <span class="text-sm font-semibold">{integration.name}</span>
            <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70">{integration.authType === 'oauth' ? 'OAuth' : integration.authType === 'none' ? 'None' : 'API Key'}</span>
            {#if integration.toolCount > 0}
              <span class="text-xs text-base-content/50">{integration.toolCount} tools</span>
            {/if}
          </div>
          <div class="text-xs font-mono text-base-content/50 truncate">{integration.serverUrl}</div>
          {#if integration.lastError}
            <div class="text-xs text-error mt-0.5">{integration.lastError}</div>
          {/if}
        </div>
        <div class="flex items-center gap-1.5 shrink-0">
          {#if integration.authType === 'oauth' && integration.connectionStatus === 'error'}
            <button
              onclick={() => reauthenticate(integration.id)}
              class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none"
              title="Reauthenticate OAuth"
            >
              <KeyRound class="w-4 h-4 text-warning" />
            </button>
          {/if}
          {#if integration.authType === 'api_key'}
            <button
              onclick={() => openKeyPrompt(integration.id)}
              class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none"
              title={integration.connectionStatus === 'connected' ? 'Replace API key' : 'Enter API key'}
            >
              <KeyRound class="w-4 h-4 {integration.connectionStatus === 'connected' ? 'text-base-content/50' : 'text-warning'}" />
            </button>
          {/if}
          <button
            onclick={() => toggleEnabled(integration.id)}
            class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none"
            title={integration.isEnabled ? 'Disconnect' : 'Connect'}
          >
            <Power class="w-4 h-4 {integration.isEnabled ? 'text-success' : 'text-base-content/30'}" />
          </button>
          <button
            onclick={() => testConnection(integration.id)}
            class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none"
            title="Test connection"
          >
            <RefreshCw class="w-4 h-4 text-base-content/50" />
          </button>
          <button
            onclick={() => removeIntegration(integration.id)}
            class="p-1.5 rounded-md hover:bg-error/10 transition-colors cursor-pointer bg-transparent border-none"
            title="Remove"
          >
            <Trash2 class="w-4 h-4 text-error/60" />
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>

<!-- Browse connectors -->
<div class="p-4 rounded-lg border border-base-300 bg-base-100">
  <div class="flex items-center justify-between">
    <div>
      <div class="text-sm font-semibold mb-0.5">Browse Connectors</div>
      <div class="text-xs text-base-content/70">Discover pre-configured MCP servers in the marketplace.</div>
    </div>
    <a
      href="/marketplace/connectors"
      class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-base-300 text-sm font-medium hover:bg-base-200 transition-colors"
    >
      Marketplace <ExternalLink class="w-3.5 h-3.5" />
    </a>
  </div>
</div>

<!-- API Key Prompt Modal -->
{#if keyPromptId}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <div class="absolute inset-0 bg-black/30" role="presentation"></div>
    <div class="relative bg-base-100 rounded-box border border-base-300 shadow-xl w-[440px] flex flex-col z-10">
      <div class="flex items-center justify-between px-5 py-3.5 border-b border-base-300 shrink-0">
        <span class="text-base font-semibold">Enter API key</span>
        <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={() => keyPromptId = null}>
          <X class="w-4 h-4" />
        </button>
      </div>
      <div class="p-5">
        <div class="text-xs text-base-content/70 mb-3">This server authenticates with a static API key or bearer token. It's stored encrypted on this device.</div>
        <input
          type="password"
          bind:value={keyPromptValue}
          placeholder="Paste your API key"
          class="input input-bordered w-full text-sm font-mono"
          onkeydown={(e) => { if (e.key === 'Enter') submitApiKey(); }}
        />
      </div>
      <div class="flex justify-end gap-2 px-5 py-3.5 border-t border-base-300">
        <button class="px-3 py-1.5 rounded-lg border border-base-300 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent" onclick={() => keyPromptId = null}>Cancel</button>
        <button class="px-3 py-1.5 rounded-lg text-sm font-medium cursor-pointer btn-primary btn" disabled={!keyPromptValue.trim()} onclick={submitApiKey}>Save & Connect</button>
      </div>
    </div>
  </div>
{/if}

<!-- Add Server Modal -->
{#if showAddModal}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <div class="absolute inset-0 bg-black/30" role="presentation"></div>
    <div class="relative bg-base-100 rounded-box border border-base-300 shadow-xl w-[520px] max-h-[80vh] flex flex-col z-10">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-3.5 border-b border-base-300 shrink-0">
        <div class="flex items-center gap-2">
          <span class="text-xs text-base-content/50 font-mono">Step {currentStep} of {totalSteps}</span>
          <span class="text-base-content/30">—</span>
          <span class="text-base font-semibold">
            {#if addStep === 'auth'}
              Add MCP Server
            {:else}
              Connect {newServerName || 'Server'}
            {/if}
          </span>
        </div>
        <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={closeAddModal}>
          <X class="w-4 h-4" />
        </button>
      </div>

      <!-- Body -->
      <div class="flex-1 overflow-y-auto p-5">
        {#if addStep === 'auth'}
          <div class="mb-3">
            <div class="text-sm font-medium mb-1">Choose authentication method</div>
            <div class="text-xs text-base-content/70">How does this server authenticate requests?</div>
          </div>
          <div class="flex flex-col gap-2">
            {#each authOptions as opt}
              <button
                class="w-full flex items-center gap-3 p-3.5 rounded-lg border cursor-pointer transition-colors text-left {newAuthType === opt.value ? 'border-primary bg-primary/10 ring-1 ring-primary/20' : 'border-base-300 bg-base-100 hover:border-base-content/30'}"
                onclick={() => newAuthType = opt.value}
              >
                <div class="w-4 h-4 rounded-full border-2 shrink-0 flex items-center justify-center {newAuthType === opt.value ? 'border-primary' : 'border-base-content/30'}">
                  {#if newAuthType === opt.value}
                    <div class="w-2 h-2 rounded-full bg-primary"></div>
                  {/if}
                </div>
                <div class="flex-1">
                  <div class="text-sm font-medium">{opt.label}</div>
                  <div class="text-xs text-base-content/70">{opt.description}</div>
                </div>
              </button>
            {/each}
          </div>

        {:else if addStep === 'configure'}
          <!-- Step 2: Configure connection (name + URL + auth-specific fields) -->
          <div class="flex flex-col gap-4">
            <label class="block">
              <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Server Name</span>
              <input type="text" bind:value={newServerName} placeholder="My MCP Server" class="w-full py-[7px] px-2.5 rounded-field border border-base-300 text-sm bg-base-100 outline-none focus:border-primary/50 transition-colors" />
            </label>

            <label class="block">
              <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Server URL</span>
              <input type="url" bind:value={newServerUrl} placeholder="https://mcp.example.com/server" class="w-full py-[7px] px-2.5 rounded-field border border-base-300 text-sm font-mono bg-base-100 outline-none focus:border-primary/50 transition-colors" />
              <span class="block text-xs text-base-content/50 mt-1">The MCP server's endpoint URL (Streamable HTTP)</span>
            </label>

            {#if newAuthType === 'api_key'}
              <label class="block">
                <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">API Key</span>
                <input type="password" bind:value={newApiKey} placeholder="sk-..." class="w-full py-[7px] px-2.5 rounded-field border border-base-300 text-sm font-mono bg-base-100 outline-none focus:border-primary/50 transition-colors" />
              </label>
            {:else if newAuthType === 'oauth'}
              <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5">
                <div class="text-xs text-base-content/70">This server uses OAuth. You'll be redirected to authorize after adding.</div>
              </div>
            {:else}
              <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5">
                <div class="text-xs text-base-content/70">No authentication required. The server will be connected directly.</div>
              </div>
            {/if}
          </div>
        {/if}
      </div>

      <!-- Footer -->
      {#if addStep === 'auth'}
        <div class="flex items-center justify-end gap-2 px-5 py-3 border-t border-base-300 shrink-0">
          <button class="btn btn-ghost btn-sm" onclick={goBack}>Back</button>
          <button class="btn btn-primary btn-sm" onclick={goNext}>Next</button>
        </div>
      {/if}
      {#if addStep === 'configure'}
        <div class="flex items-center justify-end gap-2 px-5 py-3 border-t border-base-300 shrink-0">
          <button class="btn btn-ghost btn-sm" onclick={goBack}>Back</button>
          <button
            class="btn btn-primary btn-sm"
            disabled={configureDisabled()}
            onclick={addIntegration}
          >{submitLabel}</button>
        </div>
      {/if}
    </div>
  </div>
{/if}
