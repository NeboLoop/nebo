<script lang="ts">
  import { onMount } from 'svelte';
  import Power from 'lucide-svelte/icons/power';
  import Plus from 'lucide-svelte/icons/plus';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import X from 'lucide-svelte/icons/x';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import type { McpIntegration } from '$lib/api/nebo';

  interface MCPIntegration { id: string; name: string; serverUrl: string; authType: 'oauth' | 'api_key' | 'none'; isEnabled: boolean; connectionStatus: 'connected' | 'disconnected' | 'error'; toolCount: number; lastConnectedAt: string; lastError: string | null }
  interface MCPRegistryEntry { id: string; name: string; description: string; authType: string; isBuiltin: boolean }

  let integrations = $state<MCPIntegration[]>([]);
  let registry = $state<MCPRegistryEntry[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [intResp, regResp] = await Promise.all([
        api.listIntegrations(),
        api.listRegistry(),
      ]);
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
      const regItems = Array.isArray(regResp?.registry) ? regResp.registry : [];
      if (regItems.length) {
        registry = regItems.map((r) => ({
          id: String(r.id ?? ''),
          name: String(r.name ?? ''),
          description: String(r.description ?? ''),
          authType: String(r.authType ?? 'oauth'),
          isBuiltin: Boolean(r.isBuiltin ?? false),
        }));
      }
    } catch { /* keep mock data */ }
  });
  let showAddModal = $state(false);
  let addStep = $state<'pick' | 'auth' | 'configure'>('pick');
  let selectedRegistry = $state<MCPRegistryEntry | null>(null);
  let newServerUrl = $state('');
  let newServerName = $state('');
  let newApiKey = $state('');
  let newAuthType = $state<'oauth' | 'api_key' | 'none'>('oauth');
  let registrySearch = $state('');

  const isCustom = $derived(selectedRegistry?.id === 'custom');
  const currentStep = $derived(addStep === 'pick' ? 1 : addStep === 'auth' ? 2 : 3);
  const totalSteps = $derived(isCustom ? 3 : 2);

  const filteredRegistry = $derived(
    registry.filter(r =>
      !integrations.some(i => i.name === r.name) &&
      (r.name.toLowerCase().includes(registrySearch.toLowerCase()) || r.description.toLowerCase().includes(registrySearch.toLowerCase()))
    )
  );

  function openAddModal() {
    showAddModal = true;
    addStep = 'pick';
    selectedRegistry = null;
    newServerUrl = '';
    newServerName = '';
    newApiKey = '';
    newAuthType = 'oauth';
    registrySearch = '';
  }

  function closeAddModal() {
    showAddModal = false;
  }

  function selectServer(reg: MCPRegistryEntry) {
    selectedRegistry = reg;
    newServerUrl = `https://mcp.neboloop.com/${reg.name.toLowerCase().replace(/\s+/g, '-')}`;
    newServerName = reg.name;
    newAuthType = reg.authType as 'oauth' | 'api_key' | 'none';
    addStep = 'configure';
  }

  function selectCustom() {
    selectedRegistry = { id: 'custom', name: '', description: '', authType: 'oauth', isBuiltin: false };
    newServerUrl = '';
    newServerName = '';
    newApiKey = '';
    newAuthType = 'oauth';
    addStep = 'auth';
  }

  function goBack() {
    if (addStep === 'configure' && isCustom) {
      addStep = 'auth';
    } else {
      addStep = 'pick';
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

  async function addIntegration() {
    if (!selectedRegistry) return;
    const name = isCustom ? newServerName : selectedRegistry.name;
    if (!name.trim() || !newServerUrl.trim()) return;
    const newItem = {
      id: `int_${Date.now()}`,
      name,
      serverUrl: newServerUrl,
      authType: newAuthType as 'oauth' | 'api_key',
      isEnabled: false,
      connectionStatus: 'disconnected' as const,
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
        integrations = integrations.map(i => i.id === newItem.id ? { ...i, id: resp.integration.id } : i);
      }
    } catch { /* local state already has the item */ }
  }

  async function toggleEnabled(id: string) {
    integrations = integrations.map(i =>
      i.id === id ? { ...i, isEnabled: !i.isEnabled, connectionStatus: i.isEnabled ? 'disconnected' as const : 'connected' as const, lastError: null } : i
    );
    try {
      const api = await import('$lib/api/nebo');
      const item = integrations.find(i => i.id === id);
      if (item?.isEnabled) {
        await api.connectIntegration(id);
      } else {
        await api.updateIntegration(id, { isEnabled: false });
      }
    } catch { /* local state already updated */ }
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
    if (isCustom && !newServerName.trim()) return true;
    if (newAuthType === 'api_key' && !newApiKey.trim()) return true;
    return false;
  });

  const submitLabel = $derived(
    newAuthType === 'oauth' ? 'Connect with OAuth' : 'Add Server'
  );
</script>

<div class="mb-6">
  <h2 class="text-lg font-bold mb-1">MCP Servers</h2>
  <p class="text-xs text-base-content/70">Manage Model Context Protocol server connections.</p>
</div>

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
          <button
            onclick={() => toggleEnabled(integration.id)}
            class="p-1.5 rounded-md hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none"
            title={integration.isEnabled ? 'Disconnect' : 'Connect'}
          >
            <Power class="w-4 h-4 {integration.isEnabled ? 'text-success' : 'text-base-content/30'}" />
          </button>
          <button
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

<!-- Add Server Modal -->
{#if showAddModal}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <div class="absolute inset-0 bg-black/30" role="presentation"></div>
    <div class="relative bg-base-100 rounded-box border border-base-300 shadow-xl w-[520px] max-h-[80vh] flex flex-col z-10">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-3.5 border-b border-base-300 shrink-0">
        <div class="flex items-center gap-2">
          {#if addStep !== 'pick'}
            <span class="text-xs text-base-content/50 font-mono">Step {currentStep} of {totalSteps}</span>
            <span class="text-base-content/30">—</span>
          {/if}
          <span class="text-base font-semibold">
            {#if addStep === 'pick'}
              Add MCP Server
            {:else if addStep === 'auth'}
              Authentication
            {:else}
              Connect {isCustom ? newServerName || 'Server' : selectedRegistry?.name}
            {/if}
          </span>
        </div>
        <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={closeAddModal}>
          <X class="w-4 h-4" />
        </button>
      </div>

      <!-- Body -->
      <div class="flex-1 overflow-y-auto p-5">
        {#if addStep === 'pick'}
          <input
            type="text"
            bind:value={registrySearch}
            placeholder="Search servers..."
            class="w-full py-[7px] px-2.5 rounded-field border border-base-300 text-sm bg-base-100 outline-none focus:border-primary/50 transition-colors mb-3"
          />
          <div class="flex flex-col gap-1">
            {#each filteredRegistry as reg}
              <button
                class="w-full flex items-center gap-3 p-3 rounded-lg border border-base-300 bg-base-100 cursor-pointer hover:border-base-content/30 transition-colors text-left"
                onclick={() => selectServer(reg)}
              >
                <div class="w-8 h-8 rounded-md bg-base-200 flex items-center justify-center text-xs font-mono font-bold shrink-0">MCP</div>
                <div class="flex-1 min-w-0">
                  <div class="text-sm font-medium">{reg.name}</div>
                  <div class="text-xs text-base-content/70">{reg.description}</div>
                </div>
                <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70 shrink-0">{reg.authType === 'oauth' ? 'OAuth' : 'API Key'}</span>
              </button>
            {/each}
            {#if filteredRegistry.length === 0}
              <div class="text-center py-6 text-xs text-base-content/50">No matching servers found.</div>
            {/if}
          </div>

          <div class="border-t border-base-300 mt-4 pt-4">
            <button
              class="w-full flex items-center gap-3 p-3 rounded-lg border border-dashed border-base-300 cursor-pointer hover:border-base-content/30 transition-colors text-left bg-transparent"
              onclick={selectCustom}
            >
              <div class="w-8 h-8 rounded-md bg-base-200 flex items-center justify-center shrink-0">
                <Plus class="w-4 h-4 text-base-content/50" />
              </div>
              <div class="flex-1">
                <div class="text-sm font-medium">Custom Server</div>
                <div class="text-xs text-base-content/70">Connect to any remote MCP server URL</div>
              </div>
            </button>
          </div>

        {:else if addStep === 'auth'}
          <!-- Step 2 (custom only): Auth method selection -->
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
          <!-- Step 3 (or 2 for registry): Configure connection -->
          <div class="flex flex-col gap-4">
            {#if isCustom}
              <label class="block">
                <span class="block text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Server Name</span>
                <input type="text" bind:value={newServerName} placeholder="My MCP Server" class="w-full py-[7px] px-2.5 rounded-field border border-base-300 text-sm bg-base-100 outline-none focus:border-primary/50 transition-colors" />
              </label>
            {/if}

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
