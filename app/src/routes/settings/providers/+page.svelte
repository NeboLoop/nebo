<!--
  Providers — Developer mode. Where the bot's AI comes from: Nebo AI, agents
  and models found on this computer (detected, unchanged), and the owner's own
  connections — one card per key, each with its own models. A connection's
  models are added by id or, when the provider lists them, picked in Browse
  models. Effort levels and what runs on what live in Intelligence Packs.
-->
<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import Plus from 'lucide-svelte/icons/plus';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import Terminal from 'lucide-svelte/icons/terminal';
  import X from 'lucide-svelte/icons/x';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import Spinner from '$lib/components/ui/Spinner.svelte';
  import Alert from '$lib/components/ui/Alert.svelte';
  import type { AuthProfile, CatalogModel, ConnectionModel } from '$lib/api/neboComponents';
  import {
    CONNECTION_KINDS,
    filterCatalog,
    groupByFamily,
    isOwnConnection,
    needsBaseUrl,
    parseContext,
    shortContext,
    shortUrl,
  } from '$lib/models/connections';

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

  // Opened remotely (phone, or the web console through NeboAI's tunnel): a
  // key typed here would cross NeboAI on its way, so keys are added only on
  // the computer running Nebo. The server says which this is.
  let keysLocalOnly = $state(false);

  const providerOptions = $derived([
    { value: 'anthropic', label: $t('settingsProviders.providerOptions.anthropic') },
    { value: 'openai', label: $t('settingsProviders.providerOptions.openai') },
    { value: 'google', label: $t('settingsProviders.providerOptions.google') },
    { value: 'deepseek', label: $t('settingsProviders.providerOptions.deepseek') },
    { value: 'ollama', label: $t('settingsProviders.providerOptions.ollama') },
  ]);

  const localProviderTypes = new Set(['ollama']);

  // Computed provider groups (local models on this computer)
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

  // --- Your providers: one card per connection ---
  const connections = $derived(providers.filter(p => isOwnConnection(p.provider)));
  /** Fixed-URL kinds with no connection yet: offered as "Add key" rows. */
  const missingKinds = $derived(
    (['anthropic', 'openai', 'google', 'openrouter'] as const).filter(k => !connections.some(c => c.provider === k)),
  );
  let connModels = $state<Record<string, ConnectionModel[]>>({});
  let connModelsError = $state<Record<string, string>>({});
  let browsable = $state<Record<string, boolean>>({});
  let openIds = $state<string[]>([]);
  let newModelId = $state<Record<string, string>>({});
  let addingModelFor = $state<string | null>(null);

  function kindLabel(kind: string): string {
    if ((CONNECTION_KINDS as readonly string[]).includes(kind)) return $t(`settingsProviders.kinds.${kind}`);
    return providerOptions.find(p => p.value === kind)?.label || kind;
  }

  /** The line under a connection's name: its kind (when the name doesn't say it) and its URL. */
  function connectionDetail(p: AuthProfile): string {
    const parts: string[] = [];
    if (needsBaseUrl(p.provider)) parts.push($t(`settingsProviders.kindsShort.${p.provider}`));
    else if (p.name !== kindLabel(p.provider)) parts.push(kindLabel(p.provider));
    const url = shortUrl(p.baseUrl);
    if (url) parts.push(url);
    return parts.join(' · ');
  }

  onMount(async () => {
    await Promise.all([loadProviders(), loadLocalModelsStatus(), loadJanusStatus()]);
    await Promise.all([loadModels(), ...connections.map(c => loadConnectionModels(c.id))]);
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
      keysLocalOnly = !!resp.keysLocalOnly;
    } catch (err: any) {
      error = err?.message || $t('settingsProviders.loadFailed');
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

  async function loadConnectionModels(id: string) {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listConnectionModels(id);
      connModels[id] = resp.models ?? [];
      delete connModelsError[id];
    } catch (err: any) {
      connModelsError[id] = err?.message || $t('settingsProviders.modelsLoadFailed');
    }
  }

  /** Browse models shows only when the provider lists its models: asked once, when the card opens. */
  async function loadBrowsable(id: string) {
    if (id in browsable) return;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.browseConnectionCatalog(id);
      browsable[id] = !!resp.browsable;
    } catch { browsable[id] = false; }
  }

  function toggleOpen(id: string) {
    if (openIds.includes(id)) {
      openIds = openIds.filter(x => x !== id);
      return;
    }
    openIds = [...openIds, id];
    if (!connModels[id]) loadConnectionModels(id);
    loadBrowsable(id);
  }

  async function testProvider(id: string) {
    testingId = id;
    testResult = null;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.testProvider(id);
      testResult = { id, success: resp.success, message: resp.message };
    } catch (err: any) {
      testResult = { id, success: false, message: err?.message || $t('settingsMcp.testFailed') };
    } finally { testingId = null; }
  }

  async function toggleProvider(profile: AuthProfile) {
    const newActive = !profile.isActive;
    profile.isActive = newActive;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateProvider(profile.id, { isActive: newActive });
    } catch (err: any) {
      profile.isActive = !newActive;
      error = err?.message || $t('settingsProviders.toggleFailedShort');
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
      error = err?.message || $t('settingsProviders.updateFailedShort');
    }
  }

  async function toggleConnectionModel(id: string, model: ConnectionModel) {
    const newActive = !model.isActive;
    model.isActive = newActive;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateConnectionModel(id, encodeURIComponent(model.modelId), { isActive: newActive });
    } catch (err: any) {
      model.isActive = !newActive;
      error = err?.message || $t('settingsProviders.updateFailedShort');
    }
  }

  async function addModelById(id: string) {
    const modelId = (newModelId[id] ?? '').trim();
    if (!modelId) return;
    addingModelFor = id;
    try {
      const api = await import('$lib/api/nebo');
      await api.addConnectionModel(id, { modelId });
      newModelId[id] = '';
      await loadConnectionModels(id);
    } catch (err: any) {
      connModelsError[id] = err?.message || $t('settingsProviders.modelAddFailed');
    } finally { addingModelFor = null; }
  }

  async function removeConnectionModel(id: string, model: ConnectionModel) {
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteConnectionModel(id, encodeURIComponent(model.modelId));
      await loadConnectionModels(id);
    } catch (err: any) {
      connModelsError[id] = err?.message || $t('settingsProviders.deleteFailedShort');
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
      error = err?.message || $t('settingsProviders.cliUpdateFailedShort');
    }
  }

  async function deleteProviderById(id: string) {
    if (!confirm($t('settingsProviders.removeConfirm'))) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteProvider(id);
      openIds = openIds.filter(x => x !== id);
      await loadProviders();
      await loadModels();
    } catch (err: any) {
      error = err?.message || $t('settingsProviders.deleteFailedShort');
    }
  }

  // --- Add / Edit provider ---
  type HandModel = { kind: string; modelId: string; contextWindow: string; vision: boolean; tools: boolean; thinking: boolean };
  const blankHand = (): HandModel => ({ kind: 'chat', modelId: '', contextWindow: '', vision: true, tools: true, thinking: false });

  let showForm = $state(false);
  /** The connection being edited; '' when adding. */
  let editingId = $state('');
  let form = $state({ name: '', provider: 'anthropic', apiKey: '', baseUrl: '' });
  let hand = $state<HandModel>(blankHand());
  let queued = $state<HandModel[]>([]);
  let isSaving = $state(false);
  let formError = $state('');

  function openAddModal(kind?: string) {
    editingId = '';
    form = { name: kind ? kindLabel(kind) : '', provider: kind ?? 'anthropic', apiKey: '', baseUrl: '' };
    hand = blankHand();
    queued = [];
    formError = '';
    showForm = true;
  }

  function openEditModal(p: AuthProfile) {
    editingId = p.id;
    form = { name: p.name, provider: p.provider, apiKey: '', baseUrl: p.baseUrl ?? '' };
    formError = '';
    showForm = true;
  }

  function closeForm() {
    showForm = false;
    formError = '';
  }

  function queueHandModel() {
    if (!hand.modelId.trim()) return;
    queued = [...queued, { ...hand, modelId: hand.modelId.trim() }];
    hand = blankHand();
  }

  function handBody(m: HandModel) {
    const capabilities = [m.vision && 'vision', m.tools && 'tools', m.thinking && 'thinking'].filter(Boolean);
    return { modelId: m.modelId, kind: m.kind, contextWindow: parseContext(m.contextWindow), capabilities };
  }

  async function saveProvider() {
    if (!form.name.trim()) { formError = $t('settingsProviders.nameRequired'); return; }
    if (needsBaseUrl(form.provider) && !form.baseUrl.trim()) { formError = $t('settingsProviders.baseUrlRequired'); return; }
    if (form.apiKey && keysLocalOnly) { formError = $t('settingsProviders.keysLocalOnly'); return; }
    if (!editingId && keysLocalOnly) { formError = $t('settingsProviders.keysLocalOnly'); return; }
    if (!editingId && !form.apiKey) { formError = $t('settingsProviders.apiKeyRequired'); return; }

    isSaving = true;
    formError = '';
    try {
      const api = await import('$lib/api/nebo');
      const baseUrl = needsBaseUrl(form.provider) ? form.baseUrl.trim() : undefined;
      if (editingId) {
        // An empty key field keeps the stored key.
        await api.updateProvider(editingId, { name: form.name.trim(), baseUrl, ...(form.apiKey ? { apiKey: form.apiKey } : {}) });
      } else {
        const created = (await api.createProvider({
          name: form.name.trim(),
          provider: form.provider,
          apiKey: form.apiKey,
          baseUrl,
        })) as { id?: string };
        const toAdd = hand.modelId.trim() ? [...queued, hand] : queued;
        if (created?.id) {
          for (const m of toAdd) await api.addConnectionModel(created.id, handBody(m));
          await loadConnectionModels(created.id);
        }
      }
      await loadProviders();
      await loadModels();
      if (editingId) await loadConnectionModels(editingId);
      closeForm();
    } catch (err: any) {
      formError = err?.message || (editingId ? $t('settingsProviders.saveFailed') : $t('settingsProviders.addFailed'));
    } finally { isSaving = false; }
  }

  // --- Browse models ---
  const CONTEXT_STEPS = [32_000, 128_000, 200_000, 1_000_000];
  let browse = $state<{
    profile: AuthProfile; loading: boolean; error: string; total: number; refreshedAt?: number;
    models: CatalogModel[]; selected: string[]; adding: boolean;
  } | null>(null);
  let bSearch = $state('');
  let bKind = $state('chat');
  let bVision = $state(false);
  let bTools = $state(false);
  let bThinking = $state(false);
  let bMinContext = $state(0);
  let bSort = $state<'newest' | 'price'>('newest');
  let searchTimer: ReturnType<typeof setTimeout> | null = null;

  const browseShown = $derived(
    browse ? filterCatalog(browse.models, { vision: bVision, tools: bTools, thinking: bThinking, minContext: bMinContext, sort: bSort }) : [],
  );
  const browseGroups = $derived(groupByFamily(browseShown));

  function openBrowse(p: AuthProfile) {
    bSearch = '';
    bKind = 'chat';
    bVision = bTools = bThinking = false;
    bMinContext = 0;
    bSort = 'newest';
    browse = { profile: p, loading: true, error: '', total: 0, models: [], selected: [], adding: false };
    fetchCatalog();
  }

  async function fetchCatalog() {
    if (!browse) return;
    const b = browse;
    b.loading = true;
    b.error = '';
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.browseConnectionCatalog(b.profile.id, bSearch.trim() || undefined, bKind);
      b.models = resp.models ?? [];
      b.total = resp.total ?? b.models.length;
      b.refreshedAt = resp.refreshedAt;
    } catch (err: any) {
      b.error = err?.message || $t('settingsProviders.browse.loadFailed');
    } finally { b.loading = false; }
  }

  function onSearchInput() {
    if (searchTimer) clearTimeout(searchTimer);
    searchTimer = setTimeout(fetchCatalog, 300);
  }

  function toggleSelected(modelId: string) {
    if (!browse) return;
    browse.selected = browse.selected.includes(modelId)
      ? browse.selected.filter(x => x !== modelId)
      : [...browse.selected, modelId];
  }

  async function addSelected() {
    if (!browse || browse.selected.length === 0) return;
    const b = browse;
    b.adding = true;
    b.error = '';
    try {
      const api = await import('$lib/api/nebo');
      for (const id of b.selected) {
        const m = b.models.find(x => x.modelId === id);
        if (!m) continue;
        await api.addConnectionModel(b.profile.id, {
          modelId: m.modelId,
          kind: m.kind,
          displayName: m.displayName,
          contextWindow: m.contextWindow,
          capabilities: m.capabilities,
        });
      }
      await loadConnectionModels(b.profile.id);
      browse = null;
    } catch (err: any) {
      b.error = err?.message || $t('settingsProviders.modelAddFailed');
    } finally { b.adding = false; }
  }

  function price(m: CatalogModel): string {
    if (!m.pricing) return '';
    const usd = (n: number) => `$${n.toLocaleString(undefined, { maximumFractionDigits: 3 })}`;
    return $t('settingsProviders.browse.price', { values: { input: usd(m.pricing.input), output: usd(m.pricing.output) } });
  }

  function refreshedLabel(at?: number): string {
    if (!at) return '';
    return $t('settingsProviders.browse.refreshed', { values: { date: new Date(at * 1000).toLocaleDateString() } });
  }
</script>

<SettingsHeader title={$t('settingsProviders.title')} description={$t('settingsProviders.pageDescription')} />

{#if loading}
  <div class="flex items-center justify-center gap-3 py-16">
    <Spinner size={20} />
    <span class="text-xs text-base-content/50">{$t('settingsProviders.loadingProviders')}</span>
  </div>
{:else}
  <div class="flex flex-col gap-6">
    {#if error}
      <Alert type="error">{error}</Alert>
    {/if}

    <!-- Nebo AI (Janus) -->
    <section>
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('settingsProviders.neboAi')}</div>
      <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
        <div class="flex items-center justify-between gap-3">
          <div class="flex items-center gap-2">
            <span class="text-sm font-medium">{$t('settingsProviders.neboAi')}</span>
            {#if janusStatus?.connected}
              <span class="flex items-center gap-1.5 text-xs text-success"><span class="w-2 h-2 rounded-full bg-success"></span>{$t('common.connected')}</span>
            {:else}
              <span class="flex items-center gap-1.5 text-xs text-base-content/50"><span class="w-2 h-2 rounded-full bg-base-content/40"></span>{$t('settingsProviders.notConnected')}</span>
            {/if}
          </div>
          {#if janusStatus?.connected}
            <a href="/settings/usage" class="text-xs text-primary hover:brightness-110 transition-all">{$t('settingsProviders.viewUsageLabel')}</a>
          {:else}
            <a href="/settings/account" class="text-xs font-medium text-primary hover:brightness-110 transition-all">{$t('oauth.connect')}</a>
          {/if}
        </div>
        <p class="text-xs text-base-content/50 mt-2">
          {janusStatus?.connected ? $t('settingsProviders.neboAiPacksLine') : $t('settingsProviders.connectManaged')}
          <a href="/settings/intelligence-packs" class="text-primary hover:brightness-110 transition-all">{$t('settingsProviders.openPacks')}</a>
        </p>
      </div>
    </section>

    <!-- CLI Providers -->
    {#if cliProviders.length > 0}
      <section>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">{$t('settingsProviders.cliProviders')}</div>
        <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
          <div class="flex flex-col gap-2">
            {#each cliProviders as cli (cli.id)}
              <div class="flex items-center justify-between py-2 px-3 rounded-md bg-base-200/50">
                <div>
                  <div class="flex items-center gap-2">
                    <Terminal class="w-3.5 h-3.5 text-base-content/50" />
                    <span class="text-sm font-medium">{cli.displayName}</span>
                  </div>
                  <span class="text-xs text-base-content/50 ms-5.5 font-mono">{cli.command}</span>
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
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('settingsProviders.localModels')}</div>
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
            <RefreshCw class="w-3 h-3 {discovering ? 'animate-spin' : ''}" /> {$t('settingsProviders.discover')}
          </button>
        </div>
        {#each localProvs as prov (prov.type)}
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4">
            <div class="flex items-center gap-2 mb-1">
              <div class="w-2 h-2 rounded-full {prov.configured ? 'bg-success' : 'bg-base-content/40'}"></div>
              <span class="text-sm font-medium">{prov.label}</span>
            </div>
            {#if prov.configured}
              <p class="text-xs text-base-content/50 ms-4 mb-3">{prov.models.length === 1 ? $t('settingsProviders.modelDetectedCount', { values: { count: prov.models.length } }) : $t('settingsProviders.modelsDetectedCount', { values: { count: prov.models.length } })}</p>
            {:else}
              <p class="text-xs text-base-content/50 ms-4 mb-3">{$t('settingsProviders.ollamaNotRunning')}</p>
            {/if}
            {#if prov.configured && prov.models.length > 0}
              <div class="flex flex-col gap-1.5">
                {#each prov.models as model (model.id)}
                  <div class="flex items-center justify-between py-1.5 px-3 rounded-md bg-base-200/50">
                    <span class="text-sm">{model.displayName}</span>
                    <div class="flex items-center gap-3">
                      <span class="text-xs text-base-content/50 font-mono">{$t('settingsProviders.contextWindow', { values: { count: model.contextWindow?.toLocaleString() || '?' } })}</span>
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

    <!-- Your providers: one card per connection -->
    <section>
      <div class="flex items-center justify-between gap-3 mb-2">
        <div class="flex items-baseline gap-2 min-w-0">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('settingsProviders.yourProviders')}</div>
          <span class="text-xs text-base-content/40 truncate">{$t('settingsProviders.yourProvidersHint')}</span>
        </div>
        <button
          type="button"
          class="flex items-center gap-1 text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer shrink-0"
          onclick={() => openAddModal()}
        >
          <Plus class="w-3.5 h-3.5" /> {$t('settingsProviders.addProvider')}
        </button>
      </div>

      <div class="flex flex-col gap-2">
        {#each connections as conn (conn.id)}
          {@const isOpen = openIds.includes(conn.id)}
          {@const list = connModels[conn.id] ?? []}
          <div class="rounded-lg border border-base-content/5 bg-base-100">
            <div class="flex items-center gap-2 p-4">
              <button
                type="button"
                class="flex items-center gap-2 min-w-0 flex-1 text-start cursor-pointer"
                onclick={() => toggleOpen(conn.id)}
                aria-expanded={isOpen}
              >
                {#if isOpen}<ChevronDown class="w-3.5 h-3.5 text-base-content/50 shrink-0" />{:else}<ChevronRight class="w-3.5 h-3.5 text-base-content/50 shrink-0" />{/if}
                <span class="w-2 h-2 rounded-full shrink-0 {testResult?.id === conn.id && !testResult.success ? 'bg-error' : conn.isActive ? 'bg-success' : 'bg-warning'}"></span>
                <span class="text-sm font-medium truncate">{conn.name}</span>
                {#if connectionDetail(conn)}
                  <span class="text-xs text-base-content/50 font-mono truncate">{connectionDetail(conn)}</span>
                {/if}
                {#if !isOpen && connModels[conn.id]}
                  <span class="text-xs text-base-content/40 shrink-0">{$t('settingsProviders.modelsCount', { values: { count: list.length } })}</span>
                {/if}
              </button>
              <div class="flex items-center gap-3 shrink-0">
                {#if testResult?.id === conn.id}
                  <span class="text-xs max-w-40 truncate {testResult.success ? 'text-success' : 'text-error'}">{testResult.message}</span>
                {/if}
                <button
                  type="button"
                  class="text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
                  onclick={() => testProvider(conn.id)}
                  disabled={testingId === conn.id}
                >
                  {#if testingId === conn.id}<Spinner size={14} />{:else}{$t('settingsProviders.test')}{/if}
                </button>
                <button
                  type="button"
                  class="text-xs text-base-content/50 hover:text-primary transition-colors cursor-pointer"
                  onclick={() => openEditModal(conn)}
                >
                  {$t('common.edit')}
                </button>
                <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={conn.isActive} onchange={() => toggleProvider(conn)} aria-label={conn.name} />
                <button
                  type="button"
                  class="text-base-content/30 hover:text-error transition-colors cursor-pointer"
                  onclick={() => deleteProviderById(conn.id)}
                  aria-label={$t('settingsProviders.removeNamed', { values: { name: conn.name } })}
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </button>
              </div>
            </div>

            {#if isOpen}
              <div class="px-4 pb-4 flex flex-col gap-3">
                {#if connModelsError[conn.id]}
                  <Alert type="error">{connModelsError[conn.id]}</Alert>
                {/if}

                {#if browsable[conn.id]}
                  <div class="flex justify-end">
                    <button type="button" class="btn btn-ghost btn-xs" onclick={() => openBrowse(conn)}>{$t('settingsProviders.browseModels')}</button>
                  </div>
                {/if}

                {#if !connModels[conn.id] && !connModelsError[conn.id]}
                  <div class="flex justify-center py-3"><Spinner size={16} /></div>
                {:else if list.length === 0}
                  <p class="text-xs text-base-content/50">{$t('settingsProviders.noModels')}</p>
                {:else}
                  <div class="flex flex-col gap-1.5">
                    {#each list as model (model.modelId)}
                      <div class="flex items-center justify-between gap-3 py-1.5 px-3 rounded-md bg-base-200/50 {!conn.isActive ? 'opacity-50' : ''}">
                        <div class="flex items-center gap-1.5 flex-wrap min-w-0">
                          <span class="text-sm font-mono truncate">{model.modelId}</span>
                          {#if model.kind === 'decision'}
                            <span class="badge badge-primary badge-outline badge-xs">{$t('settingsProviders.tagDecision')}</span>
                          {/if}
                          {#if model.capabilities?.includes('vision')}<span class="badge badge-ghost badge-xs">{$t('settingsProviders.tagImages')}</span>{/if}
                          {#if model.capabilities?.includes('tools')}<span class="badge badge-ghost badge-xs">{$t('settingsProviders.tagTools')}</span>{/if}
                          {#if model.capabilities?.includes('thinking')}<span class="badge badge-ghost badge-xs">{$t('settingsProviders.tagThinks')}</span>{/if}
                          {#if model.source === 'added'}
                            <span class="badge badge-outline badge-xs">{$t('settingsProviders.tagAddedByHand')}</span>
                          {/if}
                        </div>
                        <div class="flex items-center gap-3 shrink-0">
                          {#if model.contextWindow}
                            <span class="text-xs text-base-content/50 font-mono">{$t('settingsProviders.contextWindow', { values: { count: model.contextWindow.toLocaleString() } })}</span>
                          {/if}
                          {#if model.source !== 'catalog'}
                            <button
                              type="button"
                              class="text-base-content/30 hover:text-error transition-colors cursor-pointer"
                              onclick={() => removeConnectionModel(conn.id, model)}
                              aria-label={$t('settingsProviders.removeModelNamed', { values: { model: model.modelId } })}
                            >
                              <Trash2 class="w-3 h-3" />
                            </button>
                          {/if}
                          <input
                            type="checkbox"
                            class="toggle toggle-sm toggle-primary"
                            checked={model.isActive}
                            disabled={!conn.isActive}
                            onchange={() => toggleConnectionModel(conn.id, model)}
                            aria-label={model.modelId}
                          />
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}

                <form
                  class="flex items-end gap-2"
                  onsubmit={(e) => { e.preventDefault(); addModelById(conn.id); }}
                >
                  <div class="flex-1">
                    <label class="text-xs font-medium text-base-content/70 mb-1 block" for="add-model-{conn.id}">{$t('settingsProviders.addModelById')}</label>
                    <input
                      id="add-model-{conn.id}"
                      type="text"
                      bind:value={newModelId[conn.id]}
                      placeholder={$t('settingsProviders.addModelByIdPlaceholder')}
                      class="input input-bordered input-sm w-full font-mono"
                    />
                  </div>
                  <button type="submit" class="btn btn-ghost btn-sm" disabled={addingModelFor === conn.id || !(newModelId[conn.id] ?? '').trim()}>
                    {#if addingModelFor === conn.id}<Spinner size={14} />{:else}{$t('settingsProviders.addModel')}{/if}
                  </button>
                </form>
                <p class="text-xs text-base-content/40">{$t('settingsProviders.addModelHint')}</p>
              </div>
            {/if}
          </div>
        {/each}

        {#each missingKinds as kind (kind)}
          <div class="rounded-lg border border-base-content/5 bg-base-100 p-4 flex items-center justify-between">
            <div class="flex items-center gap-2">
              <span class="w-2 h-2 rounded-full bg-base-content/40"></span>
              <span class="text-sm font-medium text-base-content/70">{kindLabel(kind)}</span>
            </div>
            <button
              type="button"
              class="text-xs font-medium text-base-content/50 hover:text-primary transition-colors cursor-pointer"
              onclick={() => openAddModal(kind)}
            >
              {$t('settingsProviders.addKeyLabel')}
            </button>
          </div>
        {/each}
      </div>
    </section>
  </div>
{/if}

<!-- Add / Edit provider -->
{#if showForm}
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
    <button type="button" class="absolute inset-0 bg-base-content/40 cursor-default" onclick={closeForm} aria-label={$t('common.close')}></button>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-lg w-full max-w-lg max-h-full flex flex-col" role="dialog" aria-modal="true" aria-labelledby="provider-form-title">
      <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10 shrink-0">
        <h3 id="provider-form-title" class="text-base font-semibold">{editingId ? $t('settingsProviders.editProvider') : $t('settingsProviders.addProvider')}</h3>
        <button type="button" onclick={closeForm} class="text-base-content/50 hover:text-base-content transition-colors cursor-pointer" aria-label={$t('common.close')}>
          <X class="w-4 h-4" />
        </button>
      </div>
      <div class="px-5 py-5 flex flex-col gap-4 overflow-y-auto">
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="provider-type">{$t('settingsApps.provider')}</label>
          <select id="provider-type" bind:value={form.provider} class="select select-bordered w-full select-sm" disabled={!!editingId}>
            {#if editingId && !(CONNECTION_KINDS as readonly string[]).includes(form.provider)}
              <option value={form.provider}>{kindLabel(form.provider)}</option>
            {/if}
            {#each CONNECTION_KINDS as kind}
              <option value={kind}>{$t(`settingsProviders.kinds.${kind}`)}</option>
            {/each}
          </select>
        </div>
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="provider-name">{$t('settingsProviders.nameLabel')}</label>
          <input id="provider-name" type="text" bind:value={form.name} placeholder={$t('settingsProviders.namePlaceholderExample')} class="input input-bordered input-sm w-full" />
          <p class="text-xs text-base-content/40 mt-1">{$t('settingsProviders.nameHint')}</p>
        </div>
        {#if needsBaseUrl(form.provider)}
          <div>
            <label class="text-xs font-medium text-base-content/70 mb-1 block" for="base-url">{$t('settingsProviders.baseUrlLabel')}</label>
            <input id="base-url" type="url" bind:value={form.baseUrl} placeholder={$t('settingsProviders.compatibleUrlPlaceholder')} class="input input-bordered input-sm w-full font-mono" />
          </div>
        {/if}
        {#if keysLocalOnly}
          <Alert type="info">{$t('settingsProviders.keysLocalOnly')}</Alert>
        {:else}
          <div>
            <label class="text-xs font-medium text-base-content/70 mb-1 block" for="api-key">{$t('onboarding.apiKey.apiKeyLabel')}</label>
            <input id="api-key" type="password" bind:value={form.apiKey} placeholder={$t('settingsProviders.apiKeyPlaceholder')} class="input input-bordered input-sm w-full font-mono" />
            {#if editingId}
              <p class="text-xs text-base-content/40 mt-1">{$t('settingsProviders.apiKeyKeepHint')}</p>
            {/if}
          </div>
        {/if}

        {#if !editingId}
          <div class="flex flex-col gap-2">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('settingsProviders.modelsSection')}</div>
            <p class="text-xs text-base-content/50">{$t('settingsProviders.modelsSectionHint')}</p>
            <div class="rounded-lg border border-base-content/10 p-3 flex flex-col gap-3">
              <span class="text-xs font-medium text-base-content/70">{$t('settingsProviders.addByHand')}</span>
              {#if queued.length > 0}
                <div class="flex flex-col gap-1">
                  {#each queued as q, i}
                    <div class="flex items-center justify-between py-1 px-2 rounded-md bg-base-200/50">
                      <span class="text-xs font-mono">{q.modelId}{q.kind === 'decision' ? ` · ${$t('settingsProviders.tagDecision')}` : ''}</span>
                      <button type="button" class="text-base-content/40 hover:text-error cursor-pointer" onclick={() => (queued = queued.filter((_, j) => j !== i))} aria-label={$t('settingsProviders.removeModelNamed', { values: { model: q.modelId } })}>
                        <X class="w-3 h-3" />
                      </button>
                    </div>
                  {/each}
                </div>
              {/if}
              <div class="grid grid-cols-[auto_1fr_8rem] max-sm:grid-cols-1 gap-2">
                <div>
                  <label class="text-xs text-base-content/50 mb-1 block" for="hand-kind">{$t('settingsProviders.kindLabel')}</label>
                  <select id="hand-kind" bind:value={hand.kind} class="select select-bordered select-sm w-full">
                    <option value="chat">{$t('settingsProviders.kindChat')}</option>
                    <option value="decision">{$t('settingsProviders.kindDecision')}</option>
                  </select>
                </div>
                <div>
                  <label class="text-xs text-base-content/50 mb-1 block" for="hand-id">{$t('settingsProviders.modelIdLabel')}</label>
                  <input id="hand-id" type="text" bind:value={hand.modelId} class="input input-bordered input-sm w-full font-mono" />
                </div>
                <div>
                  <label class="text-xs text-base-content/50 mb-1 block" for="hand-ctx">{$t('settingsProviders.contextWindowLabel')}</label>
                  <input id="hand-ctx" type="text" inputmode="numeric" bind:value={hand.contextWindow} placeholder="131,072" class="input input-bordered input-sm w-full font-mono" />
                </div>
              </div>
              <div class="flex flex-wrap gap-4">
                <label class="flex items-center gap-2 text-xs cursor-pointer"><input type="checkbox" class="checkbox checkbox-xs" bind:checked={hand.vision} />{$t('settingsProviders.seesImages')}</label>
                <label class="flex items-center gap-2 text-xs cursor-pointer"><input type="checkbox" class="checkbox checkbox-xs" bind:checked={hand.tools} />{$t('settingsProviders.usesTools')}</label>
                <label class="flex items-center gap-2 text-xs cursor-pointer"><input type="checkbox" class="checkbox checkbox-xs" bind:checked={hand.thinking} />{$t('settingsProviders.thinks')}</label>
              </div>
              <p class="text-xs text-base-content/40">{$t('settingsProviders.byHandHint')}</p>
              <div>
                <button type="button" class="btn btn-ghost btn-xs" onclick={queueHandModel} disabled={!hand.modelId.trim()}>{$t('settingsProviders.addModel')}</button>
              </div>
            </div>
          </div>
        {/if}

        {#if formError}
          <Alert type="error">{formError}</Alert>
        {/if}
      </div>
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10 shrink-0">
        <button type="button" class="btn btn-ghost btn-sm" onclick={closeForm}>{$t('common.cancel')}</button>
        <button type="button" class="btn btn-primary btn-sm" onclick={saveProvider} disabled={isSaving || (!editingId && keysLocalOnly)}>
          {#if isSaving}<Spinner size={14} />{/if}
          {editingId ? $t('common.save') : $t('settingsProviders.addProvider')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Browse models -->
{#if browse}
  <div class="fixed inset-0 z-50 flex items-center justify-center p-4">
    <button type="button" class="absolute inset-0 bg-base-content/40 cursor-default" onclick={() => (browse = null)} aria-label={$t('common.close')}></button>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-lg w-full max-w-2xl max-h-full flex flex-col" role="dialog" aria-modal="true" aria-labelledby="browse-title">
      <div class="flex items-start justify-between gap-3 px-5 py-4 border-b border-base-content/10 shrink-0">
        <div>
          <h3 id="browse-title" class="text-base font-semibold">{$t('settingsProviders.browse.title', { values: { name: browse.profile.name } })}</h3>
          <p class="text-xs text-base-content/50 mt-0.5">
            {$t('settingsProviders.browse.summary', { values: { count: browse.total } })}{#if browse.refreshedAt}{' · '}{refreshedLabel(browse.refreshedAt)}{/if}
          </p>
        </div>
        <button type="button" onclick={() => (browse = null)} class="text-base-content/50 hover:text-base-content transition-colors cursor-pointer" aria-label={$t('common.close')}>
          <X class="w-4 h-4" />
        </button>
      </div>

      <div class="px-5 pt-4 pb-3 flex flex-col gap-3 border-b border-base-content/10 shrink-0">
        <div>
          <label class="text-xs font-medium text-base-content/70 mb-1 block" for="browse-search">{$t('common.search')}</label>
          <input id="browse-search" type="search" bind:value={bSearch} oninput={onSearchInput} class="input input-bordered input-sm w-full" />
        </div>
        <div class="flex flex-wrap items-end gap-2">
          <div>
            <label class="text-xs text-base-content/50 mb-1 block" for="browse-kind">{$t('settingsProviders.kindLabel')}</label>
            <select id="browse-kind" bind:value={bKind} onchange={fetchCatalog} class="select select-bordered select-sm">
              <option value="chat">{$t('settingsProviders.kindChat')}</option>
              <option value="decision">{$t('settingsProviders.kindDecision')}</option>
            </select>
          </div>
          <button type="button" class="btn btn-sm {bVision ? 'btn-primary' : 'btn-ghost'}" aria-pressed={bVision} onclick={() => (bVision = !bVision)}>{$t('settingsProviders.seesImages')}</button>
          <button type="button" class="btn btn-sm {bTools ? 'btn-primary' : 'btn-ghost'}" aria-pressed={bTools} onclick={() => (bTools = !bTools)}>{$t('settingsProviders.usesTools')}</button>
          <button type="button" class="btn btn-sm {bThinking ? 'btn-primary' : 'btn-ghost'}" aria-pressed={bThinking} onclick={() => (bThinking = !bThinking)}>{$t('settingsProviders.thinks')}</button>
          <div>
            <label class="text-xs text-base-content/50 mb-1 block" for="browse-ctx">{$t('settingsProviders.browse.context')}</label>
            <select id="browse-ctx" bind:value={bMinContext} class="select select-bordered select-sm">
              <option value={0}>{$t('settingsProviders.browse.contextAny')}</option>
              {#each CONTEXT_STEPS as step}
                <option value={step}>{$t('settingsProviders.browse.contextAtLeast', { values: { size: shortContext(step) } })}</option>
              {/each}
            </select>
          </div>
          <div>
            <label class="text-xs text-base-content/50 mb-1 block" for="browse-sort">{$t('settingsProviders.browse.sort')}</label>
            <select id="browse-sort" bind:value={bSort} class="select select-bordered select-sm">
              <option value="newest">{$t('settingsProviders.browse.sortNewest')}</option>
              <option value="price">{$t('settingsProviders.browse.sortPrice')}</option>
            </select>
          </div>
        </div>
        <span class="text-xs text-base-content/50">{$t('settingsProviders.browse.showing', { values: { shown: browseShown.length, total: browse.total } })}</span>
      </div>

      <div class="px-5 py-3 overflow-y-auto flex-1 min-h-0 flex flex-col gap-4">
        {#if browse.error}
          <Alert type="error">{browse.error}</Alert>
        {/if}
        {#if browse.loading}
          <div class="flex justify-center py-8"><Spinner size={18} /></div>
        {:else if browseGroups.length === 0}
          <p class="text-xs text-base-content/50 py-6 text-center">{$t('settingsProviders.browse.empty')}</p>
        {:else}
          {#each browseGroups as group (group.family)}
            <div class="flex flex-col gap-1">
              <div class="flex items-baseline justify-between gap-2">
                <span class="text-xs font-semibold text-base-content/70">{group.family}</span>
                <span class="text-xs text-base-content/40 font-mono">{group.vendors.join(', ')}</span>
              </div>
              {#each group.models as m (m.modelId)}
                <label class="flex items-center gap-3 py-1.5 px-3 rounded-md bg-base-200/50 {m.added ? 'opacity-60' : 'cursor-pointer'}">
                  <input
                    type="checkbox"
                    class="checkbox checkbox-xs"
                    checked={m.added || browse.selected.includes(m.modelId)}
                    disabled={m.added}
                    onchange={() => toggleSelected(m.modelId)}
                  />
                  <span class="flex flex-col min-w-0 flex-1">
                    <span class="text-sm truncate">{m.displayName}</span>
                    <span class="text-xs text-base-content/50 font-mono truncate">{m.modelId}{#if price(m)}{' · '}{price(m)}{/if}</span>
                  </span>
                  {#if m.added}
                    <span class="badge badge-ghost badge-xs shrink-0">{$t('settingsProviders.browse.added')}</span>
                  {/if}
                  {#if m.contextWindow}
                    <span class="text-xs text-base-content/50 font-mono shrink-0">{$t('settingsProviders.contextWindow', { values: { count: shortContext(m.contextWindow) } })}</span>
                  {/if}
                </label>
              {/each}
            </div>
          {/each}
        {/if}
      </div>

      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10 shrink-0">
        <span class="text-xs text-base-content/50 me-auto">{$t('settingsProviders.browse.selected', { values: { count: browse.selected.length } })}</span>
        <button type="button" class="btn btn-ghost btn-sm" onclick={() => (browse = null)}>{$t('common.cancel')}</button>
        <button type="button" class="btn btn-primary btn-sm" onclick={addSelected} disabled={browse.adding || browse.selected.length === 0}>
          {#if browse.adding}<Spinner size={14} />{/if}
          {$t('settingsProviders.browse.addN', { values: { count: browse.selected.length } })}
        </button>
      </div>
    </div>
  </div>
{/if}
