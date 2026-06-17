<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { getContext, onDestroy } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { getActivityType } from '$lib/utils/workflowTypes';
  import type { AgentPageContext, WorkflowConfig, WorkflowActivity } from '$lib/types/agentPage';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { createChatController } from '$lib/chat/controller.svelte';
  import ChatPane from '$lib/components/chat/ChatPane.svelte';
  import SetupWizard from '$lib/components/SetupWizard.svelte';
  import Check from 'lucide-svelte/icons/check';
  import MemoryManager from '$lib/components/settings/MemoryManager.svelte';
  import type { AgentInputField } from '$lib/types/agentPage';
  import { installFlow } from '$lib/stores/installFlow';
  import type { PluginAccount } from '$lib/api/pluginAccounts';

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const agentColor = $derived(ctx.agentColor);
  const skills = $derived(ctx.skills);
  const config = $derived(ctx.config);
  const workflowEntries = $derived(ctx.workflowEntries);
  const workflowStats = $derived(ctx.workflowStats);
  const devMode = $derived(ctx.devMode);

  const section = $derived($page.params.section);

  function createNewWorkflow() {
    const existing = workflowEntries.map(([name]: [string, WorkflowConfig]) => name);
    let idx = 1;
    let name = 'New Workflow';
    while (existing.includes(name)) {
      idx++;
      name = `New Workflow ${idx}`;
    }
    const wf = {
      trigger: { type: 'manual' as const },
      description: '',
      isActive: true,
      activities: [],
    };
    ctx.openWorkflow(name, wf);
  }

  const settingsSections = [
    { id: 'general', label: 'General' },
    { id: 'identity', label: 'Identity' },
    { id: 'persona', label: 'Persona' },
    { id: 'soul', label: 'Soul' },
    { id: 'rules', label: 'Rules' },
    { id: 'configure', label: 'Configure' },
    { id: 'workflows', label: 'Workflows' },
    { id: 'skills', label: 'Skills' },
    { id: 'channels', label: 'Channels' },
    { id: 'accounts', label: 'Connected Accounts' },
    { id: 'memory', label: 'Memory' },
    { id: 'permissions', label: 'Permissions' },
  ];

  // Delete confirmation triggered by ?delete=1 query param or button click
  let showDeleteConfirm = $state(false);
  let deleting = $state(false);

  $effect(() => {
    if ($page.url.searchParams.get('delete') === '1') {
      showDeleteConfirm = true;
    }
  });

  async function handleDeleteAgent() {
    if (!agentId || deleting) return;
    deleting = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteAgent(agentId);
      goto('/');
    } catch {
      deleting = false;
    }
  }

  function statusLabel(s: string) {
    if (s === 'online') return 'Online';
    if (s === 'running') return 'Running';
    if (s === 'paused') return 'Paused';
    return 'Idle';
  }

  function triggerSummary(wf: WorkflowConfig): string {
    if (wf.trigger?.type === 'schedule') return wf.schedule || 'Scheduled';
    if (wf.trigger?.type === 'event') return `On ${wf.trigger.event || 'event'}`;
    if (wf.trigger?.type === 'watch') return `Watch: ${wf.trigger.event || wf.trigger.plugin || 'plugin'}`;
    if (wf.trigger?.type === 'heartbeat') return `Every ${wf.trigger.interval || '?'}`;
    return 'Manual trigger';
  }

  function formatLastFired(iso: string): string {
    const d = new Date(iso);
    return isNaN(d.getTime()) ? iso : d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });
  }

  // --- Identity auto-save ---
  let identitySaved = $state(false);
  let identitySaveTimer: ReturnType<typeof setTimeout> | null = null;
  let editName = $state('');
  let editRole = $state('');
  let editColor = $state('');
  let editLoopExposed = $state(false);

  // Initialize edit fields only when switching to a different agent — NOT on every
  // re-emit of `agent` (saving broadcasts agent_updated → agent re-emits, which would
  // otherwise clobber what the user is currently typing and revert the name).
  let loadedIdentityFor = $state('');
  $effect(() => {
    if (agent && agentId !== loadedIdentityFor) {
      loadedIdentityFor = agentId;
      editName = agent.name;
      editRole = agent.role;
      editColor = agent.color;
      editLoopExposed = agent.loopExposed ?? false;
    }
  });

  async function saveLoopExposed() {
    if (!agentId) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateAgent(agentId, { loopExposed: editLoopExposed });
    } catch { /* silent */ }
  }

  function debounceIdentitySave() {
    if (identitySaveTimer) clearTimeout(identitySaveTimer);
    identitySaveTimer = setTimeout(() => saveIdentity(), 800);
  }

  function selectColor(color: string) {
    if (!agent?.editable) return;
    editColor = color;
    debounceIdentitySave();
  }

  async function saveIdentity() {
    if (!agentId || !agent?.editable) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateAgent(agentId, {
        name: editName,
        description: editRole,
        color: editColor,
      });
      identitySaved = true;
      setTimeout(() => identitySaved = false, 2000);
    } catch { /* silent */ }
  }

  // --- Persona auto-save (AGENT.md body) ---
  let personaSaved = $state(false);
  let personaSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let editPersona = $state('');

  $effect(() => { editPersona = config.persona; });

  function debouncePersonaSave() {
    if (personaSaveTimer) clearTimeout(personaSaveTimer);
    personaSaveTimer = setTimeout(() => savePersona(), 800);
  }

  async function savePersona() {
    if (!agentId || !agent?.editable) return;
    try {
      const api = await import('$lib/api/nebo');
      const existingMd = config.agentMd || '';
      const match = existingMd.match(/^---\n[\s\S]*?\n---\n?/);
      const newMd = match ? match[0] + '\n' + editPersona + '\n' : `---\nname: "${editName}"\ndescription: "${editRole}"\n---\n\n${editPersona}\n`;
      await api.updateAgent(agentId, { agentMd: newMd });
      personaSaved = true;
      setTimeout(() => personaSaved = false, 2000);
    } catch { /* silent */ }
  }

  // --- Soul auto-save (voice, tone, personality, boundaries) ---
  let soulSaved = $state(false);
  let soulSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let editSoul = $state('');

  $effect(() => { editSoul = config.soul; });

  function debounceSoulSave() {
    if (soulSaveTimer) clearTimeout(soulSaveTimer);
    soulSaveTimer = setTimeout(() => saveSoul(), 800);
  }

  async function saveSoul() {
    if (!agentId || !agent?.editable) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateAgent(agentId, { soul: editSoul });
      soulSaved = true;
      setTimeout(() => soulSaved = false, 2000);
    } catch { /* silent */ }
  }

  // --- Rules auto-save ---
  let rulesSaved = $state(false);
  let rulesSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let editRules = $state('');

  $effect(() => { editRules = config.rules; });

  function debounceRulesSave() {
    if (rulesSaveTimer) clearTimeout(rulesSaveTimer);
    rulesSaveTimer = setTimeout(() => saveRules(), 800);
  }

  async function saveRules() {
    if (!agentId || !agent?.editable) return;
    try {
      const api = await import('$lib/api/nebo');
      await api.updateAgent(agentId, { rules: editRules });
      rulesSaved = true;
      setTimeout(() => rulesSaved = false, 2000);
    } catch { /* silent */ }
  }

  // --- Configure (agent.json inputs) ---
  // Read-only display of the agent's saved inputs. Editing happens in the ONE
  // shared install/configure modal (installFlow store) — the same modal used
  // for install everywhere, so there's a single edit surface and a clear Save.
  let configValues = $state<Record<string, unknown>>({});
  const configFields = $derived((config.inputs ?? []) as AgentInputField[]);
  let loadedConfigFor = $state('');
  async function loadConfigValues(id: string) {
    try {
      const api = await import('$lib/api/nebo');
      const res = await api.getAgent(id);
      const raw = (res as { inputValues?: unknown })?.inputValues;
      const parsed = typeof raw === 'string' ? JSON.parse(raw || '{}') : (raw ?? {});
      configValues = parsed && typeof parsed === 'object' ? parsed : {};
    } catch {
      configValues = {};
    }
  }
  $effect(() => {
    if (!agentId || agentId === loadedConfigFor) return;
    loadedConfigFor = agentId;
    void loadConfigValues(agentId);
  });

  function openConfigure() {
    const id = agentId;
    installFlow.open({
      mode: 'configure',
      existingAgentId: id,
      agentName: agent?.name ?? '',
      oncomplete: () => void loadConfigValues(id),
    });
  }

  // --- Channels ---
  type AuthHelp = { url?: string; urlLabel?: string; text?: string };
  type ChannelInfo = { pluginSlug: string; name: string; description: string; enabled: boolean; authenticated: boolean; needsAuth: boolean; authLabel: string; authEnvKeys: string[]; authHelp?: AuthHelp | null; setup?: unknown | null; savedValues?: Record<string, string> | null };
  let channelList = $state<ChannelInfo[]>([]);
  let channelsLoading = $state(false);
  let channelTogglingSlug = $state<string | null>(null);
  let channelConnectingSlug = $state<string | null>(null);
  let channelAuthModal = $state<ChannelInfo | null>(null);
  let channelAuthInputs = $state<Record<string, string>>({});
  let channelAuthSaving = $state(false);
  let channelAuthError = $state<string | null>(null);
  let channelWizardOpen = $state(false);
  let helpChatOpen = $state(false);
  let helpChatLoading = $state(false);
  let helpChat = $state<ReturnType<typeof createChatController> | null>(null);
  let helpSessionKey = $state<string | null>(null);

  $effect(() => { if (section === 'channels') loadChannels(); });

  // Listen for plugin auth WS events when on channels section
  const channelAuthUnsubs: (() => void)[] = [];

  $effect(() => {
    if (section !== 'channels') return;
    const ws = getWebSocketClient();
    channelAuthUnsubs.push(
      // plugin_auth_url is opened once, globally, in listeners.ts — not here.
      ws.on('plugin_auth_complete', (data: Record<string, unknown>) => {
        const slug = data.plugin as string;
        if (slug) {
          channelConnectingSlug = null;
          channelAuthModal = null;
          channelAuthError = null;
          channelList = channelList.map(ch => ch.pluginSlug === slug ? { ...ch, authenticated: true } : ch);
        }
      }),
      ws.on('plugin_auth_error', (data: Record<string, unknown>) => {
        const slug = data.plugin as string;
        if (slug === channelConnectingSlug) {
          channelConnectingSlug = null;
          channelAuthError = 'Authentication failed. Check your credentials and try again.';
        }
      }),
    );
  });

  onDestroy(() => channelAuthUnsubs.forEach(fn => fn()));

  async function loadChannels() {
    channelsLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgentChannels(agentId) as { channels: ChannelInfo[] };
      channelList = resp.channels ?? [];
    } catch { channelList = []; }
    finally { channelsLoading = false; }
  }

  function openAuthModal(ch: ChannelInfo) {
    channelAuthModal = ch;
    channelAuthInputs = {};
    channelAuthError = null;
  }

  function closeAuthModal() {
    if (channelAuthSaving || channelConnectingSlug) return;
    channelAuthModal = null;
    channelAuthError = null;
    closeHelpChat();
  }

  async function submitAuthForm(slug: string) {
    channelAuthSaving = true;
    channelAuthError = null;
    try {
      const api = await import('$lib/api/nebo');
      // Save credentials per-agent so each agent gets its own bot identity
      await api.setAgentChannelConfig(agentId, slug, channelAuthInputs);
      // Mark as authenticated and auto-enable
      channelList = channelList.map(ch =>
        ch.pluginSlug === slug ? { ...ch, authenticated: true } : ch
      );
      closeAuthModal();
      await loadChannels();
    } catch {
      channelAuthError = 'Failed to save credentials.';
    }
    finally { channelAuthSaving = false; }
  }

  // Setup-wizard completion: persist the credentials it collected to this
  // agent's channel binding (same per-agent path as the manual form), then
  // close everything and refresh.
  async function onChannelWizardComplete(slug: string, envValues: Record<string, string>) {
    const api = await import('$lib/api/nebo');
    await api.setAgentChannelConfig(agentId, slug, envValues);
    channelList = channelList.map(ch =>
      ch.pluginSlug === slug ? { ...ch, authenticated: true } : ch
    );
    channelWizardOpen = false;
    channelAuthModal = null;
    await loadChannels();
  }

  async function toggleChannel(slug: string, currentlyEnabled: boolean) {
    channelTogglingSlug = slug;
    try {
      const api = await import('$lib/api/nebo');
      if (currentlyEnabled) {
        await api.disableAgentChannel(agentId, slug);
      } else {
        await api.enableAgentChannel(agentId, slug);
      }
      channelList = channelList.map(ch => ch.pluginSlug === slug ? { ...ch, enabled: !currentlyEnabled } : ch);
    } catch { /* silent */ }
    finally { channelTogglingSlug = null; }
  }

  async function openHelpChat(slug: string) {
    if (helpChatOpen) return;
    helpChatLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.startHelpChat(slug, { agentId }) as { sessionKey: string; chatId: string; agentId: string };
      if (resp.sessionKey) {
        helpSessionKey = resp.sessionKey;
        helpChat = createChatController({
          agentId: resp.agentId || agentId,
          sessionKey: resp.sessionKey,
          channel: `help:${slug}`,
        });
        // Load the seeded messages (system context + greeting)
        try {
          const msgs = await api.getSessionMessages(resp.sessionKey) as { messages?: { id: string; role: string; content: string; html?: string }[] };
          if (msgs?.messages?.length) {
            helpChat.setMessages(msgs.messages
              .filter((m) => m.role === 'user' || m.role === 'assistant')
              .map((m) => ({
                id: m.id,
                type: m.role as 'user' | 'assistant',
                content: m.content,
                html: m.html || undefined,
              })));
          }
        } catch { /* first visit */ }
        helpChatOpen = true;
      }
    } catch { /* silent */ }
    finally { helpChatLoading = false; }
  }

  function closeHelpChat() {
    helpChatOpen = false;
    if (helpChat) {
      helpChat.destroy();
      helpChat = null;
    }
    helpSessionKey = null;
  }

  // --- Connected Accounts (multi-account plugins) ---
  // Some plugins (e.g. Gmail) let one agent connect several accounts. The
  // plugins list does not expose the multi-account flag, so we list the
  // agent's auth-capable plugins and, for each, fetch its connected accounts.
  // A plugin appears here when it already has >=1 account OR the user adds one.
  type AccountPlugin = { slug: string; name: string; description: string; accounts: PluginAccount[] };
  let accountPlugins = $state<AccountPlugin[]>([]);
  let accountsLoading = $state(false);
  let addAccountPlugin = $state<AccountPlugin | null>(null);
  let addAccountLabel = $state('');
  let addAccountConnectingSlug = $state<string | null>(null);
  let addAccountError = $state<string | null>(null);

  $effect(() => { if (section === 'accounts') loadAccounts(); });

  const accountAuthUnsubs: (() => void)[] = [];

  $effect(() => {
    if (section !== 'accounts') return;
    const ws = getWebSocketClient();
    accountAuthUnsubs.push(
      // plugin_auth_url is opened once, globally, in listeners.ts — not here.
      ws.on('plugin_auth_complete', (data: Record<string, unknown>) => {
        const slug = data.plugin as string;
        if (slug && slug === addAccountConnectingSlug) {
          addAccountConnectingSlug = null;
          addAccountPlugin = null;
          addAccountLabel = '';
          addAccountError = null;
        }
        if (slug) refreshPluginAccounts(slug);
      }),
      ws.on('plugin_auth_error', (data: Record<string, unknown>) => {
        const slug = data.plugin as string;
        if (slug === addAccountConnectingSlug) {
          addAccountConnectingSlug = null;
          addAccountError = (data.error as string) || 'Sign-in failed. Please try again.';
        }
      }),
    );
    return () => { accountAuthUnsubs.forEach(fn => fn()); accountAuthUnsubs.length = 0; };
  });

  async function loadAccounts() {
    accountsLoading = true;
    try {
      const api = await import('$lib/api/nebo');
      const accountsApi = await import('$lib/api/pluginAccounts');
      const resp = await api.listPlugins() as { plugins: { slug: string; name?: string; description?: string; hasAuth?: boolean; multiAccount?: boolean }[] };
      // Only plugins that declare profile_dir_env support multiple accounts
      // per agent (the "resource" model, e.g. gws). Identity-model plugins
      // (one bot per agent, e.g. Slack) are managed under Channels, not here.
      const candidates = (resp.plugins ?? []).filter(p => p.multiAccount);
      const loaded = await Promise.all(candidates.map(async (p) => {
        let accounts: PluginAccount[] = [];
        try {
          const r = await accountsApi.listPluginAccounts(p.slug, agentId);
          accounts = r.accounts ?? [];
        } catch { /* plugin may not support multi-account */ }
        return { slug: p.slug, name: p.name || p.slug, description: p.description || '', accounts };
      }));
      // Surface plugins that already have connected accounts first; keep the
      // rest so the user can add a first account to a multi-account plugin.
      accountPlugins = loaded.sort((a, b) => b.accounts.length - a.accounts.length || a.name.localeCompare(b.name));
    } catch { accountPlugins = []; }
    finally { accountsLoading = false; }
  }

  async function refreshPluginAccounts(slug: string) {
    try {
      const accountsApi = await import('$lib/api/pluginAccounts');
      const r = await accountsApi.listPluginAccounts(slug, agentId);
      accountPlugins = accountPlugins.map(p =>
        p.slug === slug ? { ...p, accounts: r.accounts ?? [] } : p
      );
    } catch { /* silent */ }
  }

  function openAddAccount(p: AccountPlugin) {
    addAccountPlugin = p;
    addAccountLabel = '';
    addAccountError = null;
  }

  function closeAddAccount() {
    if (addAccountConnectingSlug) return;
    addAccountPlugin = null;
    addAccountLabel = '';
    addAccountError = null;
  }

  async function submitAddAccount() {
    const p = addAccountPlugin;
    const label = addAccountLabel.trim();
    if (!p || !label || addAccountConnectingSlug) return;
    addAccountConnectingSlug = p.slug;
    addAccountError = null;
    try {
      const accountsApi = await import('$lib/api/pluginAccounts');
      await accountsApi.startPluginAccountLogin(p.slug, agentId, label);
      // Login runs in the background; completion arrives via WS.
    } catch (e) {
      addAccountConnectingSlug = null;
      addAccountError = (e as Error)?.message || 'Could not start sign-in for this account.';
    }
  }
</script>

<div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
  <span class="text-sm font-semibold">{agent?.name} &mdash; {settingsSections.find(s => s.id === section)?.label ?? 'Settings'}</span>
</div>
<div class="flex-1 overflow-y-auto p-6">
  <div class="max-w-[480px] flex flex-col gap-5">

    {#if section === 'general'}
      {@const gc = agent ? AGENT_COLORS_MAP[agent.color] : null}
      <div class="flex items-start gap-4 pb-5 border-b border-base-300">
        <div class="w-12 h-12 rounded-field flex items-center justify-center font-mono text-base font-semibold shrink-0 {gc?.bgClass} {gc?.inkClass}">{agent?.initial}</div>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <div class="text-sm font-semibold">{agent?.name}</div>
            {#if !agent?.editable}
              <span class="py-0.5 px-2 rounded bg-base-200 font-mono text-xs text-base-content/70">Read-only</span>
            {/if}
          </div>
          <div class="text-xs text-base-content/70">{agent?.role}</div>
          <div class="flex items-center gap-2 mt-1.5">
            <div class="w-[7px] h-[7px] rounded-full shrink-0 {ctx.agentStatus(agentId) === 'online' ? 'bg-success' : ctx.agentStatus(agentId) === 'running' ? 'bg-warning animate-pulse' : 'bg-base-content/30'}"></div>
            <span class="text-xs text-base-content/50">{statusLabel(ctx.agentStatus(agentId))}</span>
            {#if agentId !== 'assistant'}
              <button
                class="ml-1 py-0.5 px-2 rounded text-xs font-medium cursor-pointer border border-base-300 bg-base-100 hover:bg-base-200 transition-colors"
                onclick={() => ctx.toggleAgentStatus(agentId)}
              >{ctx.agentStatus(agentId) === 'paused' ? 'Activate' : 'Pause'}</button>
            {/if}
          </div>
        </div>
      </div>

      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5">
          <div class="text-xs text-base-content/70">This agent's configuration is managed by its <span class="font-mono">agent.json</span> and cannot be edited directly. Duplicate it to create an editable copy.</div>
        </div>
      {/if}

      {#if devMode}
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Model</div>
          <div class="text-sm font-mono">{config.model}</div>
        </div>
      {/if}

      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Skills</div>
        <div class="text-sm">{skills.length > 0 ? skills.join(', ') : 'None assigned'}</div>
      </div>

      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Workflows</div>
        <div class="text-sm">{workflowEntries.length} configured</div>
      </div>

      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Created</div>
        <div class="text-sm">Mar 12, 2026</div>
      </div>

      <!-- Danger zone -->
      {#if agent?.editable}
        <div class="border-t border-base-300 pt-5 mt-3">
          <div class="text-xs font-semibold uppercase tracking-wider text-error mb-2">Danger Zone</div>
          {#if showDeleteConfirm}
            <div class="rounded-lg border border-error/30 bg-error/5 p-4">
              <div class="text-sm font-medium mb-1">Delete {agent?.name}?</div>
              <div class="text-xs text-base-content/70 mb-3">This will permanently remove the agent, all threads, runs, and memory. This action cannot be undone.</div>
              <div class="flex items-center gap-2">
                <button class="btn btn-error btn-sm" onclick={handleDeleteAgent} disabled={deleting}>{deleting ? 'Deleting...' : 'Delete Agent'}</button>
                <button class="btn btn-ghost btn-sm" onclick={() => showDeleteConfirm = false}>Cancel</button>
              </div>
            </div>
          {:else}
            <button class="btn btn-error btn-sm btn-outline" onclick={() => showDeleteConfirm = true}>Delete Agent</button>
          {/if}
        </div>
      {/if}

    {:else if section === 'identity'}
      <div class="flex items-center justify-between mb-1">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Identity</div>
        {#if identitySaved}
          <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
        {/if}
      </div>
      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5 text-xs text-base-content/70">Managed by <span class="font-mono">AGENT.md</span> — read-only.</div>
      {/if}
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">Agent Name</span>
        <input type="text" bind:value={editName} oninput={debounceIdentitySave} disabled={!agent?.editable} class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body disabled:opacity-60 disabled:cursor-not-allowed" />
      </label>
      <label class="block">
        <span class="block text-xs font-semibold uppercase tracking-wider mb-1.5">Role</span>
        <textarea bind:value={editRole} oninput={debounceIdentitySave} disabled={!agent?.editable} rows="3" class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body disabled:opacity-60 disabled:cursor-not-allowed resize-none"></textarea>
      </label>
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider mb-1.5">Color</div>
        <div class="flex gap-2">
          {#each ['violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach'] as color}
            {@const c = AGENT_COLORS_MAP[color]}
            <button
              class="w-7 h-7 rounded-md border-2 transition-colors {c.bgClass} {editColor === color ? 'border-base-content' : 'border-transparent'} {agent?.editable ? 'cursor-pointer' : 'opacity-60 cursor-not-allowed'}"
              title={color}
              disabled={!agent?.editable}
              onclick={() => selectColor(color)}
            ></button>
          {/each}
        </div>
      </div>
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider mb-1.5">Status</div>
        <div class="flex items-center gap-1.5 text-sm">
          <div class="w-[7px] h-[7px] rounded-full shrink-0 {(agent?.status ?? 'idle') === 'online' ? 'bg-success' : (agent?.status ?? 'idle') === 'running' ? 'bg-warning animate-pulse' : 'bg-base-content/30'}"></div>
          {statusLabel(agent?.status ?? 'idle')}
        </div>
      </div>

    {:else if section === 'persona'}
      <div class="flex items-center justify-between mb-1">
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Persona</div>
          <div class="text-xs text-base-content/70 mt-1">The agent's operating instructions &mdash; what it does, how it works, what rules it follows.</div>
        </div>
        {#if personaSaved}
          <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
        {/if}
      </div>
      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5 text-xs text-base-content/70">Managed by <span class="font-mono">AGENT.md</span> &mdash; read-only.</div>
      {/if}
      <textarea rows="20"
        bind:value={editPersona}
        oninput={debouncePersonaSave}
        disabled={!agent?.editable}
        placeholder={"You are a helpful assistant that specializes in...\n\n## Rules\n- Always confirm before taking consequential actions\n- Be concise but thorough when needed\n\n## Capabilities\n- Research and analysis\n- Document drafting\n- Task automation"}
        class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-mono leading-relaxed disabled:opacity-60 disabled:cursor-not-allowed"
      ></textarea>

    {:else if section === 'soul'}
      <div class="flex items-center justify-between mb-1">
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Soul</div>
          <div class="text-xs text-base-content/70 mt-1">Who this agent IS &mdash; voice, tone, personality, values, and boundaries. Not what it does, but how it speaks and who it is.</div>
        </div>
        {#if soulSaved}
          <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
        {/if}
      </div>
      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5 text-xs text-base-content/70">Managed externally &mdash; read-only.</div>
      {/if}
      <textarea rows="20"
        bind:value={editSoul}
        oninput={debounceSoulSave}
        disabled={!agent?.editable}
        placeholder={"# Core Truths\n- Be genuinely helpful, not performatively helpful\n- Have opinions and share them when relevant\n- Be resourceful before asking for help\n- Earn trust through competence\n\n# Vibe\n- Conversational and warm, not corporate\n- Direct and honest — skip filler words\n- Concise when needed, thorough when it matters\n\n# Boundaries\n- Private things stay private. Period.\n- When in doubt, ask before acting externally\n- Never send half-baked replies\n\n# Personality\n- Curious and engaged\n- Patient with complex problems\n- Lighthearted when appropriate\n- Not a sycophant — be genuine"}
        class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-mono leading-relaxed disabled:opacity-60 disabled:cursor-not-allowed"
      ></textarea>

    {:else if section === 'rules'}
      <div class="flex items-center justify-between mb-1">
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Rules</div>
          <div class="text-xs text-base-content/70 mt-1">Behavior constraints and guardrails &mdash; what this agent must or must not do.</div>
        </div>
        {#if rulesSaved}
          <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> Saved</span>
        {/if}
      </div>
      {#if !agent?.editable}
        <div class="rounded-lg border border-base-300 bg-base-200/50 px-3.5 py-2.5 text-xs text-base-content/70">Managed externally &mdash; read-only.</div>
      {/if}
      <textarea rows="20"
        bind:value={editRules}
        oninput={debounceRulesSave}
        disabled={!agent?.editable}
        placeholder={"# Rules\n- Always confirm before taking destructive actions\n- Never send emails without explicit approval\n- Keep responses concise unless asked for detail\n- Escalate to owner if uncertain about a request\n\n# Boundaries\n- Do not access files outside the project directory\n- Do not make purchases or financial commitments\n- Do not share sensitive information externally"}
        class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-mono leading-relaxed disabled:opacity-60 disabled:cursor-not-allowed"
      ></textarea>

    {:else if section === 'configure'}
      <div class="flex items-center justify-between gap-3 mb-1">
        <div class="text-sm">These inputs customize how {agent?.name} operates.</div>
        {#if configFields.length > 0}
          <button type="button" class="btn btn-sm btn-primary shrink-0" onclick={openConfigure}>Configure</button>
        {/if}
      </div>

      {#if configFields.length === 0}
        <div class="text-center py-6 text-sm">No configurable inputs for this agent.</div>
      {:else}
        <dl class="flex flex-col gap-3 mt-3">
          {#each configFields as field (field.key)}
            {@const saved = configValues[field.key]}
            {@const val = saved === undefined || saved === null || saved === '' ? field.default : saved}
            {@const isEmpty = val === undefined || val === null || val === ''}
            <div class="border-b border-base-content/10 pb-3 last:border-0">
              <dt class="text-sm font-medium">{field.label || field.key}</dt>
              {#if field.description}<dd class="text-xs text-base-content/50 mt-0.5">{field.description}</dd>{/if}
              <dd class="text-sm mt-1 whitespace-pre-wrap {isEmpty ? 'text-base-content/40 italic' : ''}">{isEmpty ? 'Not set' : String(val)}</dd>
            </div>
          {/each}
        </dl>
      {/if}

    {:else if section === 'workflows'}
      <!-- Stats cards -->
      {#if workflowStats.totalRuns > 0}
        <div class="grid grid-cols-4 gap-2 mb-4">
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold">{workflowStats.totalRuns}</div>
            <div class="text-xs text-base-content/50">Total runs</div>
          </div>
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold text-success">{workflowStats.completed}</div>
            <div class="text-xs text-base-content/50">Completed</div>
          </div>
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold {workflowStats.failed > 0 ? 'text-error' : ''}">{workflowStats.failed}</div>
            <div class="text-xs text-base-content/50">Failed</div>
          </div>
          <div class="rounded-lg border border-base-300 bg-base-100 p-2.5 text-center">
            <div class="text-base font-semibold font-mono">{workflowStats.avgDuration}</div>
            <div class="text-xs text-base-content/50">Avg duration</div>
          </div>
        </div>
      {/if}

      <!-- Header with canvas button -->
      <div class="flex items-center justify-between mb-3">
        <div class="text-sm">Automated sequences for {agent?.name}.</div>
        {#if workflowEntries.length > 0}
          <button
            class="flex items-center gap-1.5 py-1 px-2.5 rounded-lg border border-base-300 text-xs font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors"
            onclick={() => ctx.openCanvas()}
            title="Open canvas editor"
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="8" y="14" width="7" height="7" rx="1"/><line x1="6.5" y1="10" x2="11.5" y2="14"/><line x1="17.5" y1="10" x2="11.5" y2="14"/></svg>
            Canvas
          </button>
        {/if}
      </div>

      {#if workflowEntries.length === 0}
        <div class="text-center py-8 text-sm">
          No workflows configured. Create one or install from the Marketplace.
        </div>
      {:else}
        <div class="flex flex-col gap-2">
          {#each workflowEntries as [name, wf]}
            {@const purchased = wf.source === 'marketplace'}
            <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
              <div class="flex items-start gap-3 p-3.5">
                <div class="w-[22px] h-[22px] rounded flex items-center justify-center text-sm shrink-0 mt-0.5 {wf.isActive !== false ? 'bg-primary/10 text-primary' : 'bg-base-200 text-base-content/40'}">
                  {#if wf.trigger?.type === 'schedule'}&#8635;{:else if wf.trigger?.type === 'event'}&#9889;{:else if wf.trigger?.type === 'watch'}&#128065;{:else if wf.trigger?.type === 'heartbeat'}&#10084;{:else}&#9654;{/if}
                </div>

                <button class="flex-1 min-w-0 text-left cursor-pointer bg-transparent border-none p-0" onclick={() => ctx.openWorkflow(name, wf)}>
                  <div class="flex items-center gap-1.5">
                    <span class="text-sm font-medium">{name}</span>
                    {#if purchased}
                      <span class="py-0 px-1.5 rounded bg-base-200 text-xs font-mono">Marketplace</span>
                    {/if}
                    {#if wf.isActive === false}
                      <span class="py-0 px-1.5 rounded bg-base-200 text-xs text-base-content/50">Paused</span>
                    {/if}
                  </div>
                  <div class="text-xs text-base-content/70 mt-0.5 truncate">{wf.description}</div>
                  <div class="flex items-center gap-2 mt-1.5 flex-wrap">
                    <span class="text-xs text-base-content/50 font-mono">{triggerSummary(wf)}</span>
                    <span class="text-xs text-base-content/30">&middot;</span>
                    <span class="text-xs text-base-content/50 font-mono inline-flex items-center gap-1">{wf.activities?.length ?? 0} {(wf.activities?.length ?? 0) === 1 ? 'activity' : 'activities'}{#each [...new Set((wf.activities ?? []).map((a: WorkflowActivity) => a.type).filter(Boolean))] as t}<span class="inline-block" title={getActivityType(t).label}>{getActivityType(t).icon}</span>{/each}</span>
                    {#if wf.lastFired}
                      <span class="text-xs text-base-content/30">&middot;</span>
                      <span class="text-xs text-base-content/50 font-mono">Last: {formatLastFired(wf.lastFired)}</span>
                    {/if}
                    {#if wf.emit}
                      <span class="text-xs text-base-content/30">&middot;</span>
                      <span class="text-xs text-accent/70 font-mono">&#8594; {wf.emit}</span>
                    {/if}
                  </div>
                </button>

                <input type="checkbox" class="toggle toggle-sm toggle-primary shrink-0 mt-1" checked={wf.isActive !== false} role="switch" aria-checked={wf.isActive !== false} onchange={() => ctx.toggleWorkflow(name)} />
              </div>
            </div>
          {/each}
        </div>
      {/if}

      <button class="mt-3 w-full py-2.5 rounded-lg border border-dashed border-base-300 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={createNewWorkflow}>+ New workflow</button>

    {:else if section === 'skills'}
      <div class="text-xs text-base-content/70 mb-2">Skills assigned to {agent?.name}. Add more from the Marketplace.</div>
      {#each skills as skill}
        <div class="flex items-center gap-2.5 py-2 px-3 rounded-lg border border-base-300 bg-base-100">
          <div class="w-7 h-7 rounded-md bg-base-200 flex items-center justify-center text-sm shrink-0">&#9889;</div>
          <span class="text-sm font-medium flex-1">{skill}</span>
          <button class="text-sm text-error cursor-pointer bg-transparent border-none hover:opacity-70">Remove</button>
        </div>
      {/each}
      <a href="/marketplace/skills" class="inline-flex items-center gap-1 text-sm text-primary font-medium mt-1">+ Add from Marketplace &#8594;</a>

    {:else if section === 'channels'}
      <div class="flex items-center gap-3 py-2.5 px-3 rounded-lg border border-base-300 bg-base-100 mb-3">
        <div class="flex-1 min-w-0">
          <div class="text-sm font-medium">Expose to Loop</div>
          <div class="text-xs text-base-content/70 mt-0.5">When off, this agent won't appear in your loop.</div>
        </div>
        <input
          type="checkbox"
          class="toggle toggle-sm toggle-primary shrink-0"
          bind:checked={editLoopExposed}
          role="switch"
          aria-checked={editLoopExposed}
          onchange={saveLoopExposed}
        />
      </div>
      <div class="text-xs text-base-content/70 mb-2">Connect {agent?.name} to external messaging platforms. Install channel plugins from the Marketplace, then enable them here.</div>
      {#if channelsLoading}
        <div class="text-xs text-base-content/50 py-6 text-center">Loading channels...</div>
      {:else if channelList.length === 0}
        <div class="py-8 text-center">
          <div class="text-sm text-base-content/50 mb-2">No channel plugins installed</div>
          <a href="/marketplace/plugins" class="inline-flex items-center gap-1 text-sm text-primary font-medium">Browse Marketplace &#8594;</a>
        </div>
      {:else}
        <div class="flex flex-col gap-2">
          {#each channelList as ch}
            <div class="flex items-center gap-3 py-2.5 px-3 rounded-lg border border-base-300 bg-base-100">
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-sm font-medium">{ch.name}</span>
                  {#if ch.needsAuth && !ch.authenticated}
                    <span class="text-xs text-warning font-medium">Setup required</span>
                  {/if}
                </div>
                {#if ch.description}
                  <div class="text-xs text-base-content/70 mt-0.5">{ch.description}</div>
                {/if}
              </div>
              {#if ch.needsAuth && !ch.authenticated}
                <button
                  class="btn btn-sm btn-outline btn-primary"
                  onclick={() => openAuthModal(ch)}
                >Connect</button>
              {:else}
                <div class="flex items-center gap-2">
                  {#if ch.needsAuth}
                    <button
                      class="btn btn-xs btn-ghost text-base-content/50"
                      title="Update credentials"
                      onclick={() => openAuthModal(ch)}
                    >
                      <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16" fill="currentColor" class="w-3.5 h-3.5"><path fill-rule="evenodd" d="M11.013 2.513a1.75 1.75 0 0 1 2.475 2.474L6.226 12.25a2.751 2.751 0 0 1-.892.596l-2.047.848a.75.75 0 0 1-.98-.98l.848-2.047a2.75 2.75 0 0 1 .596-.892l7.262-7.262Z" clip-rule="evenodd" /></svg>
                    </button>
                  {/if}
                  <input
                    type="checkbox"
                    class="toggle toggle-sm toggle-primary shrink-0"
                    checked={ch.enabled}
                    disabled={channelTogglingSlug === ch.pluginSlug}
                    role="switch"
                    aria-checked={ch.enabled}
                    onchange={() => toggleChannel(ch.pluginSlug, ch.enabled)}
                  />
                </div>
              {/if}
            </div>
          {/each}
        </div>
        <a href="/marketplace/plugins" class="inline-flex items-center gap-1 text-sm text-primary font-medium mt-2">+ Add from Marketplace &#8594;</a>
      {/if}

    {:else if section === 'accounts'}
      <div class="text-xs text-base-content/70 mb-2">Connect multiple accounts to {agent?.name} for plugins that support it &mdash; for example several Gmail inboxes. Each account signs in separately and {agent?.name} can use any of them.</div>
      {#if accountsLoading}
        <div class="text-xs text-base-content/50 py-6 text-center">Loading accounts...</div>
      {:else if accountPlugins.length === 0}
        <div class="py-8 text-center">
          <div class="text-sm text-base-content/50 mb-2">No plugins available for multiple accounts</div>
          <a href="/marketplace/plugins" class="inline-flex items-center gap-1 text-sm text-primary font-medium">Browse Marketplace &#8594;</a>
        </div>
      {:else}
        <div class="flex flex-col gap-3">
          {#each accountPlugins as plugin (plugin.slug)}
            <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
              <div class="flex items-start gap-3 p-3.5">
                <div class="flex-1 min-w-0">
                  <div class="text-sm font-medium">{plugin.name}</div>
                  {#if plugin.description}
                    <div class="text-xs text-base-content/70 mt-0.5">{plugin.description}</div>
                  {/if}
                </div>
                <button
                  class="btn btn-sm btn-outline btn-primary shrink-0"
                  onclick={() => openAddAccount(plugin)}
                >+ Add account</button>
              </div>
              {#if plugin.accounts.length > 0}
                <div class="border-t border-base-content/10">
                  {#each plugin.accounts as acct (acct.accountLabel)}
                    <div class="flex items-center gap-2 px-3.5 py-2 border-b border-base-content/5 last:border-b-0">
                      <Check class="w-3.5 h-3.5 text-success shrink-0" />
                      <span class="text-sm truncate flex-1">{acct.accountLabel}</span>
                      {#if acct.isPrimary}
                        <span class="py-0.5 px-2 rounded bg-accent/15 text-accent text-xs font-medium shrink-0">Primary</span>
                      {/if}
                    </div>
                  {/each}
                </div>
              {:else}
                <div class="border-t border-base-content/10 px-3.5 py-2">
                  <span class="text-xs text-base-content/50">No accounts connected yet</span>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

    {:else if section === 'memory'}
      <MemoryManager {agentId} />

    {:else if section === 'permissions'}
      <div class="text-xs text-base-content/70 mb-2">Control what {agent?.name} can access and execute.</div>
      {#each [
        { id: 'files', label: 'File Access', desc: 'Read and write files on disk', on: true },
        { id: 'shell', label: 'Shell', desc: 'Execute terminal commands', on: true },
        { id: 'web', label: 'Web', desc: 'Make HTTP requests', on: true },
        { id: 'contacts', label: 'Contacts', desc: 'Access address book', on: false },
        { id: 'desktop', label: 'Desktop', desc: 'Screen capture and control', on: false },
      ] as perm}
        <div class="flex items-center gap-3 py-2">
          <div class="flex-1">
            <div class="text-sm font-medium">{perm.label}</div>
            <div class="text-xs text-base-content/70">{perm.desc}</div>
          </div>
          <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={perm.on} role="switch" aria-checked={perm.on} />
        </div>
      {/each}

    {:else}
      <div class="text-center py-10 text-sm">Unknown settings section.</div>
    {/if}

  </div>
</div>

<!-- Channel Auth Modal -->
{#if channelAuthModal}
  {@const ch = channelAuthModal}
  {@const busy = channelAuthSaving || channelConnectingSlug === ch.pluginSlug}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_interactive_supports_focus a11y_no_noninteractive_tabindex -->
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40" tabindex="-1" onkeydown={(e) => { if (e.key === 'Escape' && !helpChatOpen) closeAuthModal(); }} role="dialog" aria-modal="true">
    <div class="bg-base-100 rounded-xl border border-base-300 shadow-xl mx-4 flex overflow-hidden transition-all duration-300 ease-out {helpChatOpen ? 'w-[80vw] h-[80vh]' : 'w-full max-w-md'}">
      <!-- Left: Setup form -->
      <div class="flex flex-col min-h-0 overflow-hidden {helpChatOpen ? 'w-1/2 border-r border-base-content/10' : 'w-full'}">
        <div class="flex items-center justify-between p-5 border-b border-base-content/10">
          <div class="min-w-0">
            <div class="text-base font-semibold">Connect {ch.name}</div>
            {#if ch.authLabel}
              <div class="text-xs text-base-content/50 mt-0.5">{ch.authLabel}</div>
            {/if}
          </div>
          <button class="btn btn-ghost btn-sm btn-square" onclick={closeAuthModal} aria-label="Close" disabled={busy}>
            <svg xmlns="http://www.w3.org/2000/svg" class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
          </button>
        </div>

        <div class="p-5 space-y-4 overflow-y-auto flex-1">
          {#if ch.setup}
            <div class="rounded-lg bg-primary/5 border border-primary/30 p-3">
              <div class="text-sm font-medium mb-1">Guided setup</div>
              <div class="text-xs text-base-content/70 mb-2.5">Generate a ready-to-paste app manifest, install it, and connect — step by step.</div>
              <button class="btn btn-sm btn-primary" onclick={() => { channelWizardOpen = true; }} disabled={busy}>
                Start setup wizard
              </button>
              <div class="text-xs text-base-content/50 mt-2">Or paste tokens manually below.</div>
            </div>
          {/if}

          {#if ch.authHelp?.text}
            <div class="rounded-lg bg-base-200/50 border border-base-300 p-3">
              <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Setup Guide</div>
              {#each ch.authHelp.text.split('\n').filter(Boolean) as line}
                <div class="text-xs text-base-content/70 leading-relaxed">{line}</div>
              {/each}
              {#if ch.authHelp.url}
                <a href={ch.authHelp.url} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 text-xs text-primary font-medium mt-2 hover:underline">{ch.authHelp.urlLabel ?? ch.authHelp.url} &#8599;</a>
              {/if}
            </div>
          {:else if ch.authHelp?.url}
            <a href={ch.authHelp.url} target="_blank" rel="noopener noreferrer" class="inline-flex items-center gap-1 text-xs text-primary font-medium hover:underline">{ch.authHelp.urlLabel ?? 'Setup documentation'} &#8599;</a>
          {/if}

          {#each ch.authEnvKeys as envKey}
            <label class="flex flex-col gap-1.5">
              <span class="text-xs font-mono text-base-content/50">{envKey}</span>
              <input
                type="password"
                class="input input-sm input-bordered w-full font-mono text-xs"
                placeholder="Paste token here..."
                value={channelAuthInputs[envKey] ?? ''}
                disabled={busy}
                oninput={(e) => { channelAuthInputs[envKey] = (e.target as HTMLInputElement).value; }}
              />
            </label>
          {/each}

          {#if channelAuthError}
            <div class="text-xs text-error">{channelAuthError}</div>
          {/if}
        </div>

        <div class="flex items-center gap-2 p-5 border-t border-base-content/10">
          {#if !helpChatOpen}
            <button
              class="btn btn-sm btn-ghost text-primary"
              onclick={() => openHelpChat(ch.pluginSlug)}
              disabled={helpChatLoading}
            >{helpChatLoading ? 'Loading...' : 'Need help?'}</button>
          {/if}
          <div class="flex-1"></div>
          <button class="btn btn-sm btn-ghost" onclick={closeAuthModal} disabled={busy}>Cancel</button>
          <button
            class="btn btn-sm btn-primary"
            disabled={busy || !ch.authEnvKeys.some(k => channelAuthInputs[k]?.trim())}
            onclick={() => submitAuthForm(ch.pluginSlug)}
          >{busy ? 'Saving...' : 'Save Credentials'}</button>
        </div>
      </div>

      <!-- Right: Help chat (shown when help is open) -->
      {#if helpChatOpen && helpChat}
        <div class="w-1/2 flex flex-col min-h-0 overflow-hidden">
          <div class="flex items-center justify-between px-4 py-3 border-b border-base-content/10 shrink-0">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{ch.name} Help</div>
            <button class="btn btn-ghost btn-xs btn-square" onclick={closeHelpChat} aria-label="Close help">
              <svg xmlns="http://www.w3.org/2000/svg" class="h-3 w-3" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
            </button>
          </div>
          <div class="flex-1 flex flex-col min-h-0">
            <ChatPane
              messages={helpChat.messages}
              agentName="{ch.name} Help"
              agentId={agentId}
              sessionId={helpSessionKey ?? ''}
              placeholder="Ask about {ch.name} setup..."
              onsend={(text) => helpChat?.send(text)}
              onstop={() => helpChat?.stop()}
              isLoading={helpChat.isLoading}
            />
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}

{#if channelWizardOpen && channelAuthModal?.setup}
  <SetupWizard
    slug={channelAuthModal.pluginSlug}
    setup={channelAuthModal.setup as any}
    initialValues={channelAuthModal.savedValues ?? {}}
    alreadySetKeys={channelAuthModal.authenticated ? channelAuthModal.authEnvKeys : []}
    onClose={() => { channelWizardOpen = false; }}
    onComplete={(envValues) => onChannelWizardComplete(channelAuthModal!.pluginSlug, envValues)}
  />
{/if}

<!-- Add Account Modal -->
{#if addAccountPlugin}
  {@const plugin = addAccountPlugin}
  {@const connecting = addAccountConnectingSlug === plugin.slug}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_interactive_supports_focus a11y_no_noninteractive_tabindex -->
  <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/40" tabindex="-1" onkeydown={(e) => { if (e.key === 'Escape') closeAddAccount(); }} role="dialog" aria-modal="true">
    <div class="bg-base-100 rounded-xl border border-base-300 shadow-xl mx-4 w-full max-w-md flex flex-col overflow-hidden">
      <div class="flex items-center justify-between p-5 border-b border-base-content/10">
        <div class="min-w-0">
          <div class="text-base font-semibold">Add {plugin.name} account</div>
          <div class="text-xs text-base-content/50 mt-0.5">Label this account, then sign in to connect it.</div>
        </div>
        <button class="btn btn-ghost btn-sm btn-square" onclick={closeAddAccount} aria-label="Close" disabled={connecting}>
          <svg xmlns="http://www.w3.org/2000/svg" class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12" /></svg>
        </button>
      </div>

      <div class="p-5 space-y-4">
        <label class="flex flex-col gap-1.5">
          <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Account label</span>
          <input
            type="text"
            class="input input-sm input-bordered w-full text-sm font-body"
            placeholder="work@acme.com"
            bind:value={addAccountLabel}
            disabled={connecting}
            onkeydown={(e) => { if (e.key === 'Enter') submitAddAccount(); }}
          />
          <span class="text-xs text-base-content/50">Usually the email address or username for the account.</span>
        </label>

        {#if connecting}
          <div class="rounded-lg bg-primary/5 border border-primary/30 p-3 text-xs text-base-content/70">A sign-in window will open. Finish signing in there &mdash; this connects automatically when it completes.</div>
        {/if}

        {#if addAccountError}
          <div class="text-xs text-error">{addAccountError}</div>
        {/if}
      </div>

      <div class="flex items-center gap-2 p-5 border-t border-base-content/10">
        <div class="flex-1"></div>
        <button class="btn btn-sm btn-ghost" onclick={closeAddAccount} disabled={connecting}>Cancel</button>
        <button
          class="btn btn-sm btn-primary"
          disabled={connecting || !addAccountLabel.trim()}
          onclick={submitAddAccount}
        >{connecting ? 'Signing in...' : 'Sign in'}</button>
      </div>
    </div>
  </div>
{/if}
