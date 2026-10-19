<script lang="ts">
  // Connectors: the MCP servers on this Nebo. The same structure as the
  // phone's: a list (icon, name, tool count, a status line that fixes it),
  // a + that browses the marketplace or adds one by its address, each
  // connector's own view with real buttons and its tools, and each tool's
  // setting (Blocked, Needs approval, Always allow), all read from the
  // connector's tools list; a setting is changed by its company Permissions
  // switch (`tool:<key>`).
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import Plus from 'lucide-svelte/icons/plus';
  import X from 'lucide-svelte/icons/x';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import ChevronRight from 'lucide-svelte/icons/chevron-right';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Store from 'lucide-svelte/icons/store';
  import Link from 'lucide-svelte/icons/link';
  import Shield from 'lucide-svelte/icons/shield';
  import SlidersHorizontal from 'lucide-svelte/icons/sliders-horizontal';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import RotateCw from 'lucide-svelte/icons/rotate-cw';
  import Activity from 'lucide-svelte/icons/activity';
  import KeyRound from 'lucide-svelte/icons/key-round';
  import Unlink from 'lucide-svelte/icons/unlink';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import type { ConnectorTool, McpIntegration, PermissionSwitch } from '$lib/api/neboComponents';
  import { openSignInLink, signInFinished } from '$lib/utils/pluginSignIn';
  import {
    allToolsEdits,
    allToolsValue,
    groupTools,
    groupValue,
    toolSummary,
    connectorIconUrls,
    connectorState,
    switchesById,
    toolValue,
    validConnectorUrl,
    type Connector,
    type ConnectorState,
  } from '$lib/connectors';

  type Tool = ConnectorTool;

  let connectors = $state<Connector[]>([]);
  let loaded = $state(false);
  /** Sign-ins this page opened, by connector id. */
  let signIns = $state<Record<string, 'waiting' | 'timedOut'>>({});
  const polls = new Map<string, ReturnType<typeof setInterval>>();
  const timeouts = new Map<string, ReturnType<typeof setTimeout>>();

  /** The connector open, and the tool setting open over it. */
  let openId = $state<string | null>(null);
  let setting = $state<{ title: string; value: string; note: string; locked: boolean; pick: (v: string) => Promise<void> } | null>(null);
  let tools = $state<Tool[]>([]);
  let family = $state('');
  let refreshing = $state(false);
  let refreshed = $state<{ ok: boolean; text: string } | null>(null);
  let switches = $state(new Map<string, PermissionSwitch>());
  let testing = $state(false);
  let notice = $state<{ ok: boolean; text: string } | null>(null);
  let copied = $state(false);
  let addMenu = $state(false);
  /** How many of each connector's icon addresses failed. */
  let failedIcons = $state<Record<string, number>>({});

  const open = $derived(connectors.find((c) => c.id === openId) ?? null);
  const stateOf = (c: Connector): ConnectorState => connectorState(c, signIns[c.id]);

  function fromApi(i: McpIntegration): Connector {
    return {
      id: i.id,
      name: i.name,
      serverUrl: i.serverUrl || '',
      authType: i.authType || 'none',
      isEnabled: i.isEnabled ?? false,
      connectionStatus: i.connectionStatus || 'disconnected',
      toolCount: i.toolCount ?? 0,
      lastError: i.lastError || null,
    };
  }

  async function load() {
    try {
      const api = await import('$lib/api/nebo');
      const r = await api.listIntegrations();
      connectors = (r?.integrations ?? []).map(fromApi);
    } catch { /* an empty list says so */ }
    loaded = true;
  }

  function patch(id: string, p: Partial<Connector>) {
    connectors = connectors.map((c) => (c.id === id ? { ...c, ...p } : c));
  }

  onMount(() => {
    load();
    if (new URLSearchParams(location.search).get('add') === '1') openAdd();
  });
  onDestroy(() => {
    for (const p of polls.values()) clearInterval(p);
    for (const p of timeouts.values()) clearTimeout(p);
  });

  function stopSignIn(id: string) {
    clearInterval(polls.get(id));
    clearTimeout(timeouts.get(id));
    polls.delete(id);
    timeouts.delete(id);
  }

  /** Open the server's sign-in and wait for the bot to finish it; after
   *  three minutes say it didn't finish instead of waiting forever. */
  async function signIn(id: string, fresh: boolean) {
    if (polls.has(id)) return;
    const api = await import('$lib/api/nebo');
    let authUrl: string | undefined;
    try {
      const r = (fresh ? await api.reauthenticateIntegration(id) : await api.getOauthUrl(id)) as { authUrl?: string };
      authUrl = r?.authUrl;
    } catch (err) {
      patch(id, { connectionStatus: 'error', lastError: err instanceof Error && err.message ? err.message : $t('settingsMcp.oauthStartFailed') });
      return;
    }
    if (!authUrl) {
      patch(id, { connectionStatus: 'error', lastError: $t('settingsMcp.oauthStartFailed') });
      return;
    }
    openSignInLink({ authUrl });
    signIns = { ...signIns, [id]: 'waiting' };
    polls.set(id, setInterval(async () => {
      try {
        const r = (await api.connectIntegration(id)) as { success?: boolean; toolCount?: number };
        if (!r?.success) return;
        stopSignIn(id);
        signInFinished();
        const { [id]: _, ...rest } = signIns;
        signIns = rest;
        patch(id, { isEnabled: true, connectionStatus: 'connected', toolCount: r.toolCount ?? 0, lastError: null });
        if (openId === id) loadTools(id);
      } catch { /* not finished yet */ }
    }, 3000));
    timeouts.set(id, setTimeout(() => {
      stopSignIn(id);
      signIns = { ...signIns, [id]: 'timedOut' };
    }, 180_000));
  }

  /** Connect, or sign in again when the sign-in is gone. */
  /** `again`: the Reconnect row, which starts a sign-in over in any state. */
  async function reconnect(id: string, again = false) {
    const c = connectors.find((x) => x.id === id);
    if (!c) return;
    if (c.authType === 'oauth' && (again || c.connectionStatus === 'needs_reauth' || signIns[id] === 'timedOut')) return signIn(id, true);
    const api = await import('$lib/api/nebo');
    try {
      const r = (await api.connectIntegration(id)) as { success?: boolean; toolCount?: number; needsReauth?: boolean; message?: string };
      if (r?.success) {
        patch(id, { isEnabled: true, connectionStatus: 'connected', toolCount: r.toolCount ?? 0, lastError: null });
        if (openId === id) loadTools(id);
        return;
      }
      if (c.authType === 'oauth') return signIn(id, !!r?.needsReauth);
      patch(id, { connectionStatus: r?.needsReauth ? 'needs_reauth' : 'error', lastError: r?.message || $t('settingsMcp.connectionFailed') });
    } catch (err) {
      if (c.authType === 'oauth') return signIn(id, false);
      patch(id, { connectionStatus: 'error', lastError: err instanceof Error ? err.message : $t('settingsMcp.connectionFailed') });
    }
  }

  /** Refresh tools: connect again and read the server's tool list anew;
   *  says whether any tool arrived, left or changed, and how many there are. */
  async function refresh(id: string) {
    refreshing = true;
    refreshed = null;
    const api = await import('$lib/api/nebo');
    try {
      const r = (await api.connectIntegration(id)) as { success?: boolean; toolCount?: number; toolsUpdated?: number; message?: string; needsReauth?: boolean };
      if (r?.success) {
        const count = r.toolCount ?? 0;
        const changed = r.toolsUpdated ?? 0;
        patch(id, { isEnabled: true, connectionStatus: 'connected', toolCount: count, lastError: null });
        refreshed = {
          ok: true,
          text: changed > 0 ? $t('connectors.toolsUpdated', { values: { changed, count } }) : $t('connectors.upToDate', { values: { count } })
        };
        await loadTools(id);
      } else {
        patch(id, { connectionStatus: r?.needsReauth ? 'needs_reauth' : 'error', lastError: r?.message || null });
        refreshed = { ok: false, text: r?.message || $t('settingsMcp.connectionFailed') };
      }
    } catch (err) {
      refreshed = { ok: false, text: err instanceof Error && err.message ? err.message : $t('settingsMcp.connectionFailed') };
    }
    refreshing = false;
  }

  async function test(id: string) {
    testing = true;
    notice = null;
    const api = await import('$lib/api/nebo');
    try {
      const r = (await api.testIntegration(id)) as { success?: boolean; message?: string; needsReauth?: boolean };
      notice = { ok: !!r?.success, text: r?.message || (r?.success ? $t('connectors.connected') : $t('settingsMcp.testFailed')) };
      if (r?.success) {
        await load();
        loadTools(id);
      } else {
        patch(id, { connectionStatus: r?.needsReauth ? 'needs_reauth' : 'error', lastError: r?.message || null });
      }
    } catch {
      notice = { ok: false, text: $t('settingsMcp.testRequestFailed') };
    }
    testing = false;
  }

  async function disconnect(id: string) {
    const c = connectors.find((x) => x.id === id);
    if (!c || !confirm($t('connectors.disconnectTitle', { values: { name: c.name } }) + '\n\n' + $t('connectors.disconnectBody'))) return;
    stopSignIn(id);
    patch(id, { isEnabled: false, connectionStatus: 'disconnected', lastError: null });
    const api = await import('$lib/api/nebo');
    try { await api.updateIntegration(id, { isEnabled: false }); } catch { await load(); }
  }

  async function remove(id: string) {
    const c = connectors.find((x) => x.id === id);
    if (!c || !confirm($t('connectors.removeTitle', { values: { name: c.name } }) + '\n\n' + $t('connectors.removeBody'))) return;
    stopSignIn(id);
    connectors = connectors.filter((x) => x.id !== id);
    openId = null;
    const api = await import('$lib/api/nebo');
    try { await api.deleteIntegration(id); } catch { await load(); }
  }

  // ── A connector's view ──────────────────────────────────────────────
  async function openConnector(id: string) {
    openId = id;
    setting = null;
    notice = null;
    refreshed = null;
    tools = [];
    loadTools(id);
  }

  async function loadTools(id: string) {
    const api = await import('$lib/api/nebo');
    try {
      const r = await api.listIntegrationTools(id);
      if (openId !== id) return;
      tools = r.tools;
      family = r.family;
      switches = switchesById(r);
    } catch { /* the view says it has no tools */ }
  }

  async function setSwitch(id: string, value: string) {
    const api = await import('$lib/api/nebo');
    await api.updateCompanyPermissions({ set: { id, value } });
  }

  const word = (v: string) =>
    v === 'custom' ? $t('connectors.custom') : v === 'deny' ? $t('connectors.blocked') : v === 'ask' ? $t('connectors.ask') : $t('connectors.always');

  function openToolSetting(tool: Tool, title = tool.title) {
    setting = {
      title,
      value: toolValue(switches, family, tool.key),
      note: toolSummary(tool.description),
      locked: switches.get(`tool:${tool.key}`)?.locked ?? false,
      pick: (v) => setSwitch(`tool:${tool.key}`, v),
    };
  }

  /** One row's setting: a tool's own, or every tool of a group's. */
  function openGroupSetting(g: { title: string; tools: Tool[] }) {
    if (g.tools.length === 1) return openToolSetting(g.tools[0], g.title);
    setting = {
      title: g.title,
      value: groupValue(switches, family, g.tools.map((x) => x.key)),
      note: g.tools.map((x) => x.title).join(', '),
      locked: g.tools.some((x) => switches.get(`tool:${x.key}`)?.locked),
      pick: async (v) => {
        for (const x of g.tools) await setSwitch(`tool:${x.key}`, v);
      },
    };
  }

  function openAllTools(c: Connector) {
    const keys = tools.map((x) => x.key);
    setting = {
      title: $t('connectors.allTools'),
      value: switches.get(`tool:${family}`)?.value ?? 'allow',
      note: $t('connectors.allToolsNote', { values: { name: c.name } }),
      locked: switches.get(`tool:${family}`)?.locked ?? false,
      pick: async (v) => {
        for (const e of allToolsEdits(switches, family, keys, v)) await setSwitch(e.id, e.value);
      },
    };
  }

  async function pickSetting(v: string) {
    if (!setting || setting.locked || v === setting.value) return;
    const before = setting.value;
    setting.value = v;
    try { await setting.pick(v); } catch { if (setting) setting.value = before; }
    if (openId) await loadTools(openId);
  }

  async function copyUrl(url: string) {
    await navigator.clipboard.writeText(url);
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }

  // API key for a connector that uses one.
  let keyFor = $state<string | null>(null);
  let keyValue = $state('');
  async function saveKey() {
    const id = keyFor;
    const apiKey = keyValue.trim();
    if (!id || !apiKey) return;
    keyFor = null;
    keyValue = '';
    const api = await import('$lib/api/nebo');
    try {
      await api.updateIntegration(id, { apiKey, authType: 'api_key', isEnabled: true });
      const r = (await api.connectIntegration(id)) as { success?: boolean; message?: string };
      await load();
      if (!r?.success) patch(id, { connectionStatus: 'error', lastError: r?.message || $t('settingsMcp.connectionFailedCheckKey') });
      if (openId === id) loadTools(id);
    } catch {
      patch(id, { connectionStatus: 'error', lastError: $t('onboarding.apiKey.saveFailed') });
    }
  }

  // ── Add custom connector ────────────────────────────────────────────
  let adding = $state(false);
  let addName = $state('');
  let addUrl = $state('');
  let addBusy = $state(false);
  let addError = $state<string | null>(null);
  let ownClient = $state(false);
  let clientId = $state('');
  let clientSecret = $state('');
  let scopes = $state('');
  let authEndpoint = $state('');
  let tokenEndpoint = $state('');
  const addReady = $derived(
    !addBusy && addName.trim() !== '' && validConnectorUrl(addUrl) &&
      (!ownClient || (clientId.trim() !== '' && authEndpoint.trim() !== '' && tokenEndpoint.trim() !== '')),
  );

  function openAdd() {
    addMenu = false;
    adding = true;
    addName = '';
    addUrl = '';
    addError = null;
    ownClient = false;
    clientId = clientSecret = scopes = authEndpoint = tokenEndpoint = '';
  }

  async function addConnector() {
    addBusy = true;
    addError = null;
    const api = await import('$lib/api/nebo');
    try {
      const r = await api.createIntegration({
        name: addName.trim(),
        serverUrl: addUrl.trim(),
        authType: ownClient ? 'oauth' : 'auto',
        oauth: ownClient
          ? {
              clientId: clientId.trim(),
              clientSecret: clientSecret.trim() || undefined,
              scopes: scopes.trim() || undefined,
              authorizationEndpoint: authEndpoint.trim(),
              tokenEndpoint: tokenEndpoint.trim(),
            }
          : undefined,
      });
      const c = fromApi(r.integration as McpIntegration);
      connectors = [...connectors, c];
      adding = false;
      openConnector(c.id);
      if (c.authType === 'oauth') signIn(c.id, false);
      else if (c.authType === 'api_key') keyFor = c.id;
      else reconnect(c.id);
    } catch (err) {
      addError = err instanceof Error && err.message ? err.message : $t('connectors.addFailed');
    }
    addBusy = false;
  }

  const statusLine = (s: ConnectorState) =>
    ({
      connected: null,
      expired: $t('connectors.statusExpired'),
      off: $t('connectors.statusOff'),
      error: $t('connectors.statusError'),
      waiting: $t('connectors.statusWaiting'),
      timedOut: $t('connectors.statusTimedOut'),
    })[s];

  const headline = (s: ConnectorState) =>
    ({
      connected: $t('connectors.connected'),
      expired: $t('connectors.headExpired'),
      off: $t('connectors.headOff'),
      error: $t('connectors.headError'),
      waiting: $t('connectors.headWaiting'),
      timedOut: $t('connectors.headTimedOut'),
    })[s];

  const authWords = (a: string) =>
    a === 'oauth' ? $t('connectors.authOauth') : a === 'api_key' ? $t('connectors.authKey') : a === 'neboai' ? $t('connectors.authNebo') : $t('connectors.authNone');
</script>

{#snippet icon(c: Connector, size: 'sm' | 'md')}
  {@const src = connectorIconUrls(c.serverUrl)[failedIcons[c.id] ?? 0]}
  <span class="connector-icon connector-icon-{size}">
    {#if src}
      <img {src} alt="" onerror={() => (failedIcons = { ...failedIcons, [c.id]: (failedIcons[c.id] ?? 0) + 1 })} />
    {:else}
      {(c.name.trim()[0] ?? '?').toUpperCase()}
    {/if}
  </span>
{/snippet}

<div class="connectors">
  {#if setting && open}
    <!-- One tool's setting, or every tool's. -->
    <header class="connectors-head">
      <button class="connectors-back" onclick={() => (setting = null)} aria-label={$t('common.back')}><ChevronLeft class="w-5 h-5" /></button>
      <h2 class="connectors-title">{setting.title}</h2>
      <span class="connectors-head-spacer"></span>
    </header>
    <div class="connector-card">
      {#each [['deny', $t('connectors.blocked'), $t('connectors.blockedDesc')], ['ask', $t('connectors.approvalTitle'), $t('connectors.approvalDesc')], ['allow', $t('connectors.allowTitle'), $t('connectors.allowDesc')]] as [v, title, desc] (v)}
        <button class="connector-choice" disabled={setting.locked} onclick={() => pickSetting(v)} data-testid="tool-setting-{v}">
          <span class="connector-choice-text">
            <span class="connector-row-name">{title}</span>
            <span class="connector-row-sub">{desc}</span>
          </span>
          {#if setting.value === v}<Check class="connector-check" />{/if}
        </button>
      {/each}
    </div>
    {#if setting.locked}<p class="connector-footnote">{$t('connectors.locked')}</p>{/if}
    {#if setting.note}<p class="connector-footnote">{setting.note}</p>{/if}
  {:else if open}
    {@const s = stateOf(open)}
    <header class="connectors-head">
      <button class="connectors-back" onclick={() => (openId = null)} aria-label={$t('common.back')}><ChevronLeft class="w-5 h-5" /></button>
      <h2 class="connectors-title">{open.name}</h2>
      <span class="connectors-head-spacer"></span>
    </header>

    {@const fix = s === 'expired' || s === 'error' || s === 'timedOut' || s === 'off'}
    <section class="connector-card connector-header">
      <div class="connector-header-top">
        {@render icon(open, 'md')}
        <div class="min-w-0 flex-1">
          <div class="connector-row-name">{open.name}</div>
          <div class="connector-header-status">
            <span class="connector-dot connector-dot-{s}"></span>
            <span><strong>{headline(s)}</strong> · {authWords(open.authType)}{#if s === 'connected' && open.toolCount > 0} · {$t('connectors.toolCount', { values: { count: open.toolCount } })}{/if}</span>
          </div>
        </div>
      </div>
      {#if fix && s !== 'off' && open.lastError}<p class="connector-error">{open.lastError}</p>{/if}
      {#if open.serverUrl}
        <button class="connector-url" onclick={() => copyUrl(open.serverUrl)} title={$t('settingsMcp.copyUrl')}>
          <span class="truncate">{open.serverUrl}</span>
          {#if copied}<Check class="w-3.5 h-3.5 text-success" />{:else}<Copy class="w-3.5 h-3.5" />{/if}
        </button>
      {/if}
      {#if fix}
        <button class="btn btn-primary w-full" onclick={() => reconnect(open.id)}>
          {s === 'off' ? $t('connectors.connect') : $t('connectors.reconnect')}
        </button>
      {/if}
    </section>

    {#if tools.length === 0}
      <p class="connector-footnote">{$t('connectors.noTools')}</p>
    {:else}
      {@const canSet = switches.has(`tool:${family}`)}
      {#if canSet}
        <div class="connector-card">
          <button class="connector-row" onclick={() => openAllTools(open)}>
            <SlidersHorizontal class="connector-row-icon" />
            <span class="connector-row-title">{$t('connectors.allTools')}</span>
            <span class="connector-value">{word(allToolsValue(switches, family, tools.map((x) => x.key)))}</span>
            <ChevronRight class="connector-chevron" />
          </button>
        </div>
      {/if}
      <h3 class="connector-section">{$t('connectors.tools')}</h3>
      <div class="connector-card">
        {#each groupTools(tools) as g (g.tools[0].key)}
          {#if canSet}
            <button class="connector-row" onclick={() => openGroupSetting(g)} title={g.tools.length === 1 ? toolSummary(g.tools[0].description) : ''}>
              {@render icon(open, 'sm')}
              <span class="connector-row-title">{g.title}</span>
              <span class="connector-value">{word(groupValue(switches, family, g.tools.map((x) => x.key)))}</span>
              <ChevronRight class="connector-chevron" />
            </button>
          {:else}
            <div class="connector-row" title={toolSummary(g.tools[0].description)}>
              {@render icon(open, 'sm')}
              <span class="connector-row-title">{g.title}</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}

    <div class="connector-card">
      <button class="connector-row" onclick={() => refresh(open.id)} disabled={refreshing}>
        <RefreshCw class="connector-row-icon" />
        <span class="connector-row-title">{$t('connectors.refreshTools')}</span>
        {#if refreshing}<span class="loading loading-spinner loading-xs"></span>{:else if refreshed}<span class={refreshed.ok ? 'connector-value' : 'connector-error'}>{refreshed.text}</span>{/if}
      </button>
      <button class="connector-row" onclick={() => reconnect(open.id, true)} disabled={s === 'waiting'}>
        <RotateCw class="connector-row-icon" />
        <span class="connector-row-title">{$t('connectors.reconnect')}</span>
        {#if s === 'waiting'}<span class="loading loading-spinner loading-xs"></span>{/if}
      </button>
      <button class="connector-row" onclick={() => test(open.id)} disabled={testing}>
        <Activity class="connector-row-icon" />
        <span class="connector-row-title">{$t('settingsMcp.testConnection')}</span>
        {#if testing}<span class="loading loading-spinner loading-xs"></span>{:else if notice}<span class={notice.ok ? 'connector-value' : 'connector-error'}>{notice.text}</span>{/if}
      </button>
      {#if open.authType === 'api_key'}
        <button class="connector-row" onclick={() => { keyFor = open.id; keyValue = ''; }}>
          <KeyRound class="connector-row-icon" />
          <span class="connector-row-title">{s === 'connected' ? $t('settingsMcp.replaceApiKey') : $t('settingsMcp.enterApiKey')}</span>
          <ChevronRight class="connector-chevron" />
        </button>
      {/if}
    </div>

    <div class="connector-card">
      {#if open.isEnabled}
        <button class="connector-row connector-row-danger" onclick={() => disconnect(open.id)}>
          <Unlink class="connector-row-icon" /><span class="connector-row-title">{$t('connectors.disconnect')}</span>
        </button>
      {/if}
      <button class="connector-row connector-row-danger" onclick={() => remove(open.id)}>
        <Trash2 class="connector-row-icon" /><span class="connector-row-title">{$t('connectors.remove')}</span>
      </button>
    </div>
  {:else}
    <header class="connectors-head">
      <span class="connectors-head-spacer"></span>
      <h2 class="connectors-title">{$t('connectors.title')}</h2>
      <div class="connectors-add">
        <button class="connectors-plus" onclick={() => (addMenu = !addMenu)} aria-label={$t('connectors.add')} title={$t('connectors.add')}><Plus class="w-5 h-5" /></button>
        {#if addMenu}
          <ul class="connectors-menu">
            <li><a href="/marketplace/connectors" onclick={() => (addMenu = false)}><Store class="w-4 h-4" />{$t('connectors.browse')}</a></li>
            <li><button onclick={openAdd}><Link class="w-4 h-4" />{$t('connectors.addCustom')}</button></li>
          </ul>
        {/if}
      </div>
    </header>

    {#if loaded && connectors.length === 0}
      <div class="connectors-empty">
        <p class="connector-row-name">{$t('connectors.empty')}</p>
        <p class="connector-row-sub">{$t('connectors.emptyBody')}</p>
        <div class="connector-actions justify-center">
          <a class="btn btn-primary btn-sm" href="/marketplace/connectors">{$t('connectors.browse')}</a>
          <button class="btn btn-outline btn-sm" onclick={openAdd}>{$t('connectors.addCustom')}</button>
        </div>
      </div>
    {:else if connectors.length > 0}
      <div class="connector-card">
        {#each connectors as c (c.id)}
          {@const s = stateOf(c)}
          {@const line = statusLine(s)}
          <div class="connector-row connector-list-row">
            <button class="connector-row-open" onclick={() => openConnector(c.id)}>
              {@render icon(c, 'md')}
              <span class="connector-row-name truncate">{c.name}</span>
            </button>
            {#if s === 'connected' && c.toolCount > 0}
              <span class="connector-pill" title={$t('connectors.toolCount', { values: { count: c.toolCount } })}>{c.toolCount}</span>
            {/if}
            <button class="connector-open-chevron" onclick={() => openConnector(c.id)} aria-label={c.name}><ChevronRight class="connector-chevron" /></button>
            {#if line}
              <!-- The status line is its own button: it fixes the connector. -->
              <button class="connector-status-line connector-status-{s}" disabled={s === 'waiting'} onclick={() => reconnect(c.id)}>{line}</button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</div>

{#if adding}
  <div class="modal modal-open" role="dialog">
    <div class="modal-box connector-add">
      <header class="connectors-head">
        <button class="connectors-back" onclick={() => (adding = false)} aria-label={$t('common.close')}><X class="w-5 h-5" /></button>
        <h2 class="connectors-title">{$t('connectors.addCustom')}</h2>
        <span class="connectors-head-spacer"></span>
      </header>
      <label class="connector-field">
        <span class="connector-field-label">{$t('connectors.name')}</span>
        <input class="input input-bordered w-full" bind:value={addName} data-testid="connector-name" />
        <span class="connector-meta">{$t('connectors.nameHelper')}</span>
      </label>
      <label class="connector-field">
        <span class="connector-field-label">{$t('connectors.url')}</span>
        <input class="input input-bordered w-full font-mono" type="url" placeholder={$t('settingsMcp.serverUrlPlaceholder')} bind:value={addUrl} data-testid="connector-url" />
        <span class="connector-meta">{$t('connectors.urlHelper')}</span>
        {#if addUrl.trim() && !validConnectorUrl(addUrl)}<span class="connector-error">{$t('connectors.urlInvalid')}</span>{/if}
      </label>
      <div class="connector-trust"><Shield class="w-4 h-4 shrink-0" /><span>{$t('connectors.trust')}</span></div>
      <button class="connector-expander" onclick={() => (ownClient = !ownClient)}>
        <span>{$t('settingsMcp.preRegisteredTitle')}</span><ChevronDown class="w-4 h-4" />
      </button>
      {#if ownClient}
        <p class="connector-meta">{$t('settingsMcp.preRegisteredHint')}</p>
        <label class="connector-field"><span class="connector-field-label">{$t('settingsMcp.oauthClientId')}</span><input class="input input-bordered w-full font-mono" bind:value={clientId} /></label>
        <label class="connector-field"><span class="connector-field-label">{$t('settingsMcp.oauthClientSecret')}</span><input class="input input-bordered w-full font-mono" type="password" bind:value={clientSecret} /></label>
        <label class="connector-field"><span class="connector-field-label">{$t('settingsMcp.oauthScopes')}</span><input class="input input-bordered w-full font-mono" placeholder={$t('settingsMcp.oauthScopesPlaceholder')} bind:value={scopes} /></label>
        <label class="connector-field"><span class="connector-field-label">{$t('settingsMcp.oauthAuthEndpoint')}</span><input class="input input-bordered w-full font-mono" type="url" bind:value={authEndpoint} /></label>
        <label class="connector-field"><span class="connector-field-label">{$t('settingsMcp.oauthTokenEndpoint')}</span><input class="input input-bordered w-full font-mono" type="url" bind:value={tokenEndpoint} /></label>
        <p class="connector-meta">{$t('settingsMcp.oauthRedirectHint')} <code class="font-mono select-all">{location.origin}/api/v1/integrations/oauth/callback</code></p>
      {/if}
      {#if addError}<p class="connector-error">{addError}</p>{/if}
      <button class="btn btn-primary w-full" disabled={!addReady} onclick={addConnector} data-testid="connector-continue">
        {#if addBusy}<span class="loading loading-spinner loading-sm"></span>{:else}{$t('connectors.continue')}{/if}
      </button>
    </div>
  </div>
{/if}

{#if keyFor}
  {@const c = connectors.find((x) => x.id === keyFor)}
  <div class="modal modal-open" role="dialog">
    <div class="modal-box connector-add">
      <h3 class="connector-row-name">{$t('connectors.keyTitle')}</h3>
      <p class="connector-row-sub">{$t('connectors.keyBody', { values: { name: c?.name ?? '' } })}</p>
      <input class="input input-bordered w-full font-mono" type="password" placeholder={$t('settingsMcp.pasteApiKey')} bind:value={keyValue} onkeydown={(e) => e.key === 'Enter' && saveKey()} />
      <div class="connector-actions justify-end">
        <button class="btn btn-ghost btn-sm" onclick={() => (keyFor = null)}>{$t('common.cancel')}</button>
        <button class="btn btn-primary btn-sm" disabled={!keyValue.trim()} onclick={saveKey}>{$t('settingsMcp.saveAndConnect')}</button>
      </div>
    </div>
  </div>
{/if}
