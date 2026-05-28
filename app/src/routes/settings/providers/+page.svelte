<script lang="ts">
  import { onMount } from 'svelte';
  import KeyRound from 'lucide-svelte/icons/key-round';
  import Plus from 'lucide-svelte/icons/plus';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import Terminal from 'lucide-svelte/icons/terminal';
  import X from 'lucide-svelte/icons/x';
  import Check from 'lucide-svelte/icons/check';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import Alert from '$lib/components/ui/Alert.svelte';
  import type { AuthProfile, ListModelsResponse } from '$lib/api/neboComponents';

  // --- state ---
  let loading = $state(true);
  let error = $state('');
  let providers = $state<AuthProfile[]>([]);
  let models = $state<Record<string, any[]>>({});
  let cliProviders = $state<any[]>([]);
  let janusStatus = $state<any>(null);
  let localStatus = $state<any>(null);

  let testingId = $state<string | null>(null);
  let testResult = $state<{ id: string; success: boolean; message: string } | null>(null);
  let discovering = $state(false);

  // Add provider form
  let showAddForm = $state(false);
  let newProvider = $state({ name: '', provider: 'anthropic', apiKey: '', baseUrl: '' });
  let isAdding = $state(false);
  let addError = $state('');

  const providerOptions = [
    { value: 'anthropic', label: 'Anthropic (Claude)' },
    { value: 'openai', label: 'OpenAI (GPT)' },
    { value: 'google', label: 'Google (Gemini)' },
    { value: 'deepseek', label: 'DeepSeek' },
    { value: 'ollama', label: 'Ollama (Local)' },
  ];

  const localProviderTypes = new Set(['ollama']);
  const isLocalProvider = $derived(newProvider.provider === 'ollama');

  // Computed provider groups
  let allProviders = $derived(() => {
    const result: {
      type: string; label: string; configured: boolean; isLocal: boolean;
      profile: AuthProfile | null; models: any[];
    }[] = [];

    const allTypes = new Set([...Object.keys(models), ...providerOptions.map(p => p.value)]);
    const cliIds = cliProviders.map((p: any) => p.id);

    for (const providerType of allTypes) {
      if (cliIds.includes(providerType)) continue;
      if (providerType === 'janus') continue;

      const label = providerOptions.find(p => p.value === providerType)?.label || providerType;
      const profile = providers.find(p => p.provider === providerType) || null;
      const provModels = models[providerType] || [];
      const isLocal = localProviderTypes.has(providerType);
      const configured = isLocal
        ? (!!localStatus?.available && provModels.length > 0)
        : !!profile;

      result.push({ type: providerType, label, configured, isLocal, profile, models: provModels });
    }

    return result.sort((a, b) => {
      if (a.configured !== b.configured) return a.configured ? -1 : 1;
      return a.label.localeCompare(b.label);
    });
  });

  let localProvs = $derived(allProviders().filter(p => p.isLocal));
  let apiProvs = $derived(allProviders().filter(p => !p.isLocal));

  // Janus models (hide embeddings)
  let janusModels = $derived(() => {
    const all = models['janus'] || [];
    return all.filter((m: any) => !/embeddings?/i.test(m.displayName || m.id));
  });

  function janusDisplayName(model: any): string {
    const name = model.displayName || model.id;
    if (/^janus$/i.test(name)) return 'Nebo AI';
    return name.replace(/^janus\s*/i, 'Nebo AI ');
  }

  onMount(async () => {
    await Promise.all([loadProviders(), loadLocalModelsStatus(), loadJanusStatus()]);
    await loadModels();
  });

  async function loadJanusStatus() {
    try {
      const api = await import('$lib/api/nebo');
      janusStatus = await api.neboAIAccountStatus();
    } catch { janusStatus = null; }
  }

  async function loadLocalModelsStatus() {
    try {
      const api = await import('$lib/api/nebo');
      localStatus = await api.localModelsStatus();
    } catch { localStatus = null; }
  }

  async function loadProviders() {
    loading = true;
    error = '';
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listProviders();
      providers = resp.profiles || [];
    } catch (err: any) {
      error = err?.message || 'Failed to load providers';
    } finally { loading = false; }
  }

  async function loadModels() {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listModels();
      models = (resp.models as Record<string, any[]>) || {};
      cliProviders = (resp.cliProviders as any[]) || [];
    } catch { /* silent */ }
  }

  async function testProvider(id: string) {
    testingId = id;
    testResult = null;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.testProvider(id);
      testResult = { id, success: resp.success, message: resp.message };
    } catch (err: any) {
      testResult = { id, success: false, message: err?.message || 'Test failed' };
    } finally { testingId = null; }
  }

  async function toggleProvider(profile: AuthProfile) {
    const newActive = !profile.isActive;
    profile.isActive = newActive;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateProvider(profile.id, { isActive: newActive });
      if (!newActive) {
        const provModels = models[profile.provider] || [];
        for (const model of provModels) {
          if (model.isActive) {
            model.isActive = false;
            api.updateModel(profile.provider, model.id, { active: false }).catch(() => {});
          }
        }
      }
    } catch (err: any) {
      profile.isActive = !newActive;
      error = err?.message || 'Toggle failed';
    }
  }

  async function toggleModel(providerType: string, model: any) {
    const newActive = !model.isActive;
    model.isActive = newActive;
    try {
      const api = await import('$lib/api/nebo');
      if (providerType === 'janus' && newActive && janusStatus?.connected && !janusStatus.janusProvider) {
        await api.updateProvider(janusStatus.profileId, { metadata: { janus_provider: 'true' } });
        await loadJanusStatus();
      }
      await api.updateModel(providerType, model.id, { active: newActive });
    } catch (err: any) {
      model.isActive = !newActive;
      error = err?.message || 'Update failed';
    }
  }

  async function toggleCLI(cli: any) {
    const newActive = !cli.active;
    cli.active = newActive;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateCliProvider(cli.id, { active: newActive });
    } catch (err: any) {
      cli.active = !newActive;
      error = err?.message || 'CLI update failed';
    }
  }

  async function deleteProviderById(id: string) {
    if (!confirm('Are you sure you want to remove this provider?')) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteProvider(id);
      await loadProviders();
      await loadModels();
    } catch (err: any) {
      error = err?.message || 'Delete failed';
    }
  }

  function openAddModal(providerType?: string) {
    if (providerType) {
      const label = providerOptions.find(p => p.value === providerType)?.label || providerType;
      newProvider = { name: `My ${label}`, provider: providerType, apiKey: '', baseUrl: '' };
    } else {
      newProvider = { name: '', provider: 'anthropic', apiKey: '', baseUrl: '' };
    }
    addError = '';
    showAddForm = true;
  }

  function closeAddModal() {
    showAddForm = false;
    newProvider = { name: '', provider: 'anthropic', apiKey: '', baseUrl: '' };
    addError = '';
  }

  async function addProvider() {
    if (!newProvider.name) { addError = 'Name is required'; return; }
    if (!isLocalProvider && !newProvider.apiKey) { addError = 'API key is required'; return; }

    isAdding = true;
    addError = '';
    try {
      const api = await import('$lib/api/nebo');
      await api.createProvider({
        name: newProvider.name,
        provider: newProvider.provider,
        apiKey: newProvider.apiKey || '',
        baseUrl: newProvider.baseUrl || undefined,
      });
      await loadProviders();
      await loadModels();
      closeAddModal();
    } catch (err: any) {
      addError = err?.message || 'Failed to add provider';
    } finally { isAdding = false; }
  }
</script>

<div class="mb-7">
  <h2 class="text-base font-semibold mb-1">Providers</h2>
  <p class="text-xs text-base-content/70">Configure LLM providers, API keys, and model availability.</p>
</div>

{#if loading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/50">Loading providers...</span>
  </div>
{:else}
  <div class="flex flex-col gap-6">
    {#if error}
      <Alert type="error">{error}</Alert>
    {/if}

    <!-- NeboAI AI (Janus) -->
    <section>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Nebo AI</div>
      <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
        {#if janusStatus?.connected}
          <div class="flex items-center justify-between mb-3">
            <span class="text-sm font-medium">NeboAI AI</span>
            <a href="/settings/usage" class="text-xs text-primary hover:brightness-110 transition-all">View Usage</a>
          </div>
          {#if janusModels().length > 0}
            <div class="flex flex-col gap-1.5">
              {#each janusModels() as model (model.id)}
                <div class="flex items-center justify-between py-1.5 px-3 rounded-md bg-base-200/50">
                  <span class="text-sm">{janusDisplayName(model)}</span>
                  <div class="flex items-center gap-3">
                    <span class="text-xs text-base-content/50 font-mono">{model.contextWindow?.toLocaleString() || '?'} ctx</span>
                    <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={model.isActive} onchange={() => toggleModel('janus', model)} />
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        {:else}
          <div class="flex items-center justify-between">
            <div>
              <span class="text-sm font-medium">Not connected</span>
              <p class="text-xs text-base-content/50 mt-0.5">Connect to NeboAI for managed AI models.</p>
            </div>
            <a href="/settings/account" class="text-xs font-medium text-primary hover:brightness-110 transition-all">Connect</a>
          </div>
        {/if}
      </div>
    </section>

    <!-- CLI Providers -->
    {#if cliProviders.length > 0}
      <section>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">CLI Providers</div>
        <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
          <div class="flex flex-col gap-2">
            {#each cliProviders as cli (cli.id)}
              <div class="flex items-center justify-between py-2 px-3 rounded-md bg-base-200/50">
                <div>
                  <div class="flex items-center gap-2">
                    <Terminal class="w-3.5 h-3.5 text-base-content/50" />
                    <span class="text-sm font-medium">{cli.displayName}</span>
                  </div>
                  <span class="text-xs text-base-content/50 ml-5.5 font-mono">{cli.command}</span>
                </div>
                <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={cli.active} onchange={() => toggleCLI(cli)} />
              </div>
            {/each}
          </div>
        </div>
      </section>
    {/if}

    <!-- Local Models -->
    {#if localProvs.length > 0}
      <section>
        <div class="flex items-center justify-between mb-2">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Local Models</div>
          <button
            type="button"
            class="flex items-center gap-1 text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
            disabled={discovering}
            onclick={async () => {
              discovering = true;
              const min = new Promise(r => setTimeout(r, 800));
              try { await Promise.all([loadLocalModelsStatus(), loadModels(), min]); }
              finally { discovering = false; }
            }}
          >
            <RefreshCw class="w-3 h-3 {discovering ? 'animate-spin' : ''}" /> Discover
          </button>
        </div>
        {#each localProvs as prov (prov.type)}
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
            <div class="flex items-center gap-2 mb-1">
              <div class="w-2 h-2 rounded-full {prov.configured ? 'bg-success' : 'bg-base-content/40'}"></div>
              <span class="text-sm font-medium">{prov.label}</span>
            </div>
            {#if prov.configured}
              <p class="text-xs text-base-content/50 ml-4 mb-3">{prov.models.length} model{prov.models.length !== 1 ? 's' : ''} detected</p>
            {:else}
              <p class="text-xs text-base-content/50 ml-4 mb-3">Ollama not detected. Make sure it's running.</p>
            {/if}
            {#if prov.configured && prov.models.length > 0}
              <div class="flex flex-col gap-1.5">
                {#each prov.models as model (model.id)}
                  <div class="flex items-center justify-between py-1.5 px-3 rounded-md bg-base-200/50">
                    <span class="text-sm">{model.displayName}</span>
                    <div class="flex items-center gap-3">
                      <span class="text-xs text-base-content/50 font-mono">{model.contextWindow?.toLocaleString() || '?'} ctx</span>
                      <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={model.isActive} onchange={() => toggleModel(prov.type, model)} />
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </section>
    {/if}

    <!-- API Key Providers -->
    <section>
      <div class="flex items-center justify-between mb-2">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">API Key Providers</div>
        <button
          type="button"
          class="flex items-center gap-1 text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
          onclick={() => openAddModal()}
        >
          <Plus class="w-3.5 h-3.5" /> Add Provider
        </button>
      </div>

      <div class="flex flex-col gap-2">
        {#each apiProvs as prov (prov.type)}
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-2">
                <div class="w-2 h-2 rounded-full {prov.configured && prov.profile?.isActive ? 'bg-success' : prov.configured ? 'bg-warning' : 'bg-base-content/40'}"></div>
                <div>
                  <span class="text-sm font-medium">{prov.profile?.name || prov.label}</span>
                  {#if prov.profile?.name && prov.profile.name !== prov.label}
                    <span class="text-xs text-base-content/50 ml-1">{prov.label}</span>
                  {/if}
                </div>
              </div>
              <div class="flex items-center gap-2">
                {#if prov.configured && prov.profile}
                  {#if testResult?.id === prov.profile.id}
                    <span class="text-xs {testResult.success ? 'text-success' : 'text-error'}">{testResult.message}</span>
                  {/if}
                  <button
                    type="button"
                    class="text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
                    onclick={() => testProvider(prov.profile!.id)}
                    disabled={testingId === prov.profile.id}
                  >
                    {#if testingId === prov.profile.id}<Spinner size={14} />{:else}Test{/if}
                  </button>
                  <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={prov.profile.isActive} onchange={() => toggleProvider(prov.profile!)} />
                  <button
                    type="button"
                    class="text-base-content/30 hover:text-error transition-colors cursor-pointer"
                    onclick={() => deleteProviderById(prov.profile!.id)}
                  >
                    <Trash2 class="w-3.5 h-3.5" />
                  </button>
                {:else}
                  <button
                    type="button"
                    class="text-xs font-medium text-base-content/50 hover:text-primary transition-colors cursor-pointer"
                    onclick={() => openAddModal(prov.type)}
                  >
                    Add Key
                  </button>
                {/if}
              </div>
            </div>

            <!-- Model toggles -->
            {#if prov.models.length > 0}
              {@const providerActive = prov.configured && prov.profile?.isActive !== false}
              <div class="flex flex-col gap-1.5 mt-3">
                {#each prov.models as model (model.id)}
                  <div class="flex items-center justify-between py-1.5 px-3 rounded-md bg-base-200/50 {!providerActive ? 'opacity-50' : ''}">
                    <span class="text-sm">{model.displayName}</span>
                    <div class="flex items-center gap-3">
                      <span class="text-xs text-base-content/50 font-mono">{model.contextWindow?.toLocaleString() || '?'} ctx</span>
                      <input
                        type="checkbox"
                        class="toggle toggle-sm toggle-primary"
                        checked={providerActive ? model.isActive : false}
                        disabled={!providerActive}
                        onchange={() => toggleModel(prov.type, model)}
                      />
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    </section>
  </div>
{/if}

<!-- Add Provider Modal -->
{#if showAddForm}
  <div class="fixed inset-0 z-50 flex items-center justify-center">
    <button type="button" class="absolute inset-0 bg-base-content/40 cursor-default" onclick={closeAddModal}></button>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-lg w-full max-w-lg" role="dialog" aria-modal="true">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-base font-semibold">Add Provider</h3>
        <button type="button" onclick={closeAddModal} class="text-base-content/50 hover:text-base-content transition-colors cursor-pointer">
          <X class="w-4 h-4" />
        </button>
      </div>
      <!-- Body -->
      <div class="px-5 py-5 flex flex-col gap-4">
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="provider-type">Provider</label>
          <select id="provider-type" bind:value={newProvider.provider} class="select select-bordered w-full select-sm">
            {#each providerOptions as opt}
              <option value={opt.value}>{opt.label}</option>
            {/each}
          </select>
        </div>
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="provider-name">Name</label>
          <input id="provider-name" type="text" bind:value={newProvider.name} placeholder="e.g. My Anthropic" class="input input-bordered input-sm w-full" />
        </div>
        {#if !isLocalProvider}
          <div>
            <label class="text-xs font-medium text-base-content/70 mb-1 block" for="api-key">API Key</label>
            <input id="api-key" type="password" bind:value={newProvider.apiKey} placeholder="sk-..." class="input input-bordered input-sm w-full font-mono" />
          </div>
        {/if}
        {#if isLocalProvider}
          <div>
            <label class="text-xs font-medium text-base-content/70 mb-1 block" for="base-url">Base URL <span class="font-normal text-base-content/40">(optional)</span></label>
            <input id="base-url" type="text" bind:value={newProvider.baseUrl} placeholder="http://localhost:11434" class="input input-bordered input-sm w-full font-mono" />
            <p class="text-xs text-base-content/40 mt-1">Defaults to http://localhost:11434 if not set.</p>
          </div>
        {/if}
        {#if addError}
          <Alert type="error">{addError}</Alert>
        {/if}
      </div>
      <!-- Footer -->
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        <button type="button" class="btn btn-ghost btn-sm" onclick={closeAddModal}>Cancel</button>
        <button type="button" class="btn btn-primary btn-sm" onclick={addProvider} disabled={isAdding}>
          {#if isAdding}<Spinner size={14} /> Adding...{:else}Add Provider{/if}
        </button>
      </div>
    </div>
  </div>
{/if}
