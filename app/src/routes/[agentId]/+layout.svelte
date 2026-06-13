<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { setContext, onMount } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import WorkflowBuilder from '$lib/components/workflow/WorkflowBuilder.svelte';
  import InstallFlowModal from '$lib/components/install/InstallFlowModal.svelte';
  import { launchApp } from '$lib/apps/launcher.js';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar.js';
  const sidebarCollapsed = sidebarCollapsedFor('agents');
  import { devMode } from '$lib/stores/devmode.js';
  import type { AgentDisplay, EnrichedChat, AgentRun, WorkflowStatsLocal, WorkflowConfig } from '$lib/types/agentPage';
  import { mapWorkflows, saveWorkflows } from '$lib/utils/workflowApi';
  import type { Agent, AgentRunEntry, ActiveAgent, WorkflowRun } from '$lib/api/neboComponents';

  /** Raw run entries from the API — WorkflowRun is the generated type, but the backend
   *  also populates AgentRunEntry fields. We use a union to cover both shapes. */
  type RawRunRecord = WorkflowRun & Partial<AgentRunEntry> & Record<string, unknown>;

  let { children } = $props();

  let allAgents = $state<AgentDisplay[]>([]);
  let apiThreads = $state<Record<string, EnrichedChat[]>>({});
  let apiRuns = $state<Record<string, AgentRun[]>>({});
  let apiRunsTotal = $state<Record<string, number>>({});
  let apiRunsLoading = $state<Record<string, boolean>>({});
  let apiRawRuns = $state<Record<string, RawRunRecord[]>>({});
  let apiStats = $state<Record<string, WorkflowStatsLocal>>({});
  let apiSkills = $state<Record<string, string[]>>({});
  let apiConfig = $state<Record<string, { persona: string; agentMd: string; soul: string; rules: string; model: string; inputs: unknown[]; workflows: Record<string, WorkflowConfig> }>>({});
  let agentsLoading = $state(true);
  let threadsLoading = $state<Record<string, boolean>>({});

  // Onboarding modal state
  let showSetupModal = $state(false);
  let showInstallModal = $state(false);
  let setupAgentName = $state('');
  let setupAgentDesc = $state('');
  let setupInputFields = $state<Record<string, unknown>[]>([]);

  const DEFAULT_CONFIG = { persona: '', agentMd: '', soul: '', rules: '', model: 'claude-sonnet-4-6', inputs: [] as unknown[], workflows: {} as Record<string, WorkflowConfig> };

  // Load agents from API and return roster-refresh function
  async function loadAgentRoster() {
    try {
      const api = await import('$lib/api/nebo');
      const [agentsResp, activeResp] = await Promise.all([
        api.listAgents(),
        api.listActiveAgents().catch((e: unknown) => { console.warn('[nebo] listActiveAgents failed:', e); return null; }),
      ]);
      const activeAgents = (activeResp?.agents || []) as ActiveAgent[];
      const activeIds = new Set<string>(
        activeAgents.map((a) => a.id || a.agentId)
      );
      if (agentsResp?.agents?.length) {
        const agents = agentsResp.agents as Agent[];
        allAgents = agents.map(a => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: activeIds.has(a.id) ? 'online' : 'paused',
          color: a.color || 'teal',
          handle: a.handle,
          editable: !a.nappPath,
          isApp: a.isApp ?? false,
          loopExposed: a.loopExposed ?? false,
        }));
        agentStatuses = Object.fromEntries(allAgents.map(a => [a.id, a.status]));
      }
    } catch (e) {
      console.error('[nebo] Failed to load agents:', e);
    } finally {
      agentsLoading = false;
    }
  }

  // Refresh threads for the currently viewed agent
  async function refreshThreads() {
    const id = $page.params.agentId;
    if (!id) return;
    try {
      const api = await import('$lib/api/nebo');
      const chatsResp = await api.listAgentChats(id).catch(() => null);
      if (chatsResp?.chats) apiThreads[id] = chatsResp.chats as EnrichedChat[];
    } catch { /* silent */ }
  }

  // Map raw WorkflowRun API objects to the AgentRun shape the UI expects
  function mapRuns(raw: RawRunRecord[]): AgentRun[] {
    return raw.map(r => {
      const startSecs = typeof r.startedAt === 'number' ? r.startedAt : 0;
      const endSecs = typeof r.completedAt === 'number' ? r.completedAt : 0;
      const durSecs = endSecs > 0 && startSecs > 0 ? endSecs - startSecs : 0;
      const durStr = durSecs > 0
        ? (durSecs >= 60 ? `${Math.floor(durSecs / 60)}m ${Math.round(durSecs % 60)}s` : `${Math.round(durSecs)}s`)
        : (r.status === 'running' ? 'running...' : '—');
      const dt = startSecs > 0 ? new Date(startSecs * 1000) : null;
      const rawName = String(r.triggerDetail || r.currentActivity || r.triggerType || 'Workflow run');
      // Extract workflow binding name: "auto-reply:gws.email.new" → "auto-reply"
      const wfName = rawName.includes(':') ? rawName.split(':')[0] : rawName;
      return {
        id: String(r.id || ''),
        name: rawName,
        workflowName: wfName,
        status: r.status === 'completed' ? 'success' : String(r.status || 'unknown'),
        duration: durStr,
        date: dt ? dt.toLocaleString() : '—',
        dateGroup: dt ? dt.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: 'numeric' }) : '—',
        time: dt ? dt.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' }) : '—',
        workflowRunId: String(r.id || ''),
        trigger: String(r.triggerType || 'manual'),
        output: typeof r.output === 'string' ? r.output : undefined,
        error: typeof r.error === 'string' ? r.error : undefined,
      };
    });
  }

  // Map raw API run objects to WFRun shape expected by the run detail page
  function mapRawRunsToWFRuns(raw: RawRunRecord[]) {
    return raw.map(r => {
      const startSecs = typeof r.startedAt === 'number' ? r.startedAt : 0;
      const endSecs = typeof r.completedAt === 'number' ? r.completedAt : 0;
      const durSecs = endSecs > 0 && startSecs > 0 ? endSecs - startSecs : 0;
      const durStr = durSecs > 0
        ? (durSecs >= 60 ? `${Math.floor(durSecs / 60)}m ${Math.round(durSecs % 60)}s` : `${Math.round(durSecs)}s`)
        : (r.status === 'running' ? 'running...' : '—');
      return {
        id: String(r.id || ''),
        triggerType: String(r.triggerType || 'manual'),
        duration: durStr,
        startedAt: startSecs > 0 ? new Date(startSecs * 1000).toLocaleString() : '—',
        completedAt: endSecs > 0 ? new Date(endSecs * 1000).toLocaleString() : '—',
        tokens: (r.totalTokensUsed && typeof r.totalTokensUsed === 'number')
          ? { input: Math.round((r.totalTokensUsed as number) * 0.7), output: Math.round((r.totalTokensUsed as number) * 0.3) }
          : undefined,
        error: r.error ? String(r.error) : undefined,
        activities: Array.isArray(r.activities) ? r.activities : undefined,
        workflowId: String(r.workflowId || ''),
      };
    });
  }

  // Refresh runs + stats for the currently viewed agent
  async function refreshRuns() {
    const id = $page.params.agentId;
    if (!id) return;
    try {
      const api = await import('$lib/api/nebo');
      const [runsResp, statsResp] = await Promise.all([
        api.listAgentRuns(id).catch(() => null),
        api.agentStats(id).catch(() => null),
      ]);
      if (runsResp?.runs) {
        const rawRuns = runsResp.runs as RawRunRecord[];
        apiRuns[id] = mapRuns(rawRuns);
        apiRawRuns[id] = rawRuns;
        apiRunsTotal[id] = typeof runsResp.total === 'number' ? runsResp.total : rawRuns.length;
      }
      if (statsResp?.stats) {
        const s = statsResp.stats;
        const secs = s.avgDurationSecs ?? 0;
        const avgStr = secs > 0 ? (secs >= 60 ? `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s` : `${Math.round(secs)}s`) : '—';
        apiStats[id] = { totalRuns: s.totalRuns ?? 0, completed: s.completed ?? 0, failed: s.failed ?? 0, running: s.running ?? 0, avgDuration: avgStr, lastRunAt: s.lastRunAt ? new Date(s.lastRunAt * 1000).toLocaleString() : '—' };
      }
    } catch { /* silent */ }
  }

  // WS event handlers — kept as named functions so we can remove them on destroy
  const wsHandlers: { event: string; handler: (e: Event) => void }[] = [];

  function onWsEvent(event: string, handler: (data: any) => void) {
    const wrapped = (e: Event) => handler((e as CustomEvent).detail);
    wsHandlers.push({ event, handler: wrapped });
    window.addEventListener(event, wrapped);
  }

  onMount(() => {
    // Initial roster load
    loadAgentRoster();

    // --- WebSocket event listeners (event-driven, no polling) ---

    // Agent lifecycle → refresh roster
    onWsEvent('nebo:agent_activated', (data) => {
      if (data?.agentId) agentStatuses[data.agentId] = 'online';
      loadAgentRoster();
    });
    onWsEvent('nebo:agent_deactivated', (data) => {
      if (data?.agentId) agentStatuses[data.agentId] = 'paused';
      loadAgentRoster();
    });
    onWsEvent('nebo:agent_installed', () => loadAgentRoster());
    onWsEvent('nebo:agent_uninstalled', () => loadAgentRoster());
    onWsEvent('nebo:agent_updated', (data) => {
      // Patch the roster in place from the broadcast payload so the sidebar row
      // and agent header reflect a rename immediately (name + avatar initial +
      // role), without waiting on a refetch round-trip. Reassign the array to
      // trigger the `sortedAgents`/`agent` derived recompute.
      if (data?.agentId) {
        const idx = allAgents.findIndex(a => a.id === data.agentId);
        if (idx !== -1) {
          const next = [...allAgents];
          const updated = { ...next[idx] };
          if (typeof data.name === 'string' && data.name) {
            updated.name = data.name;
            updated.initial = data.name.charAt(0).toUpperCase();
          }
          if (typeof data.description === 'string') updated.role = data.description;
          next[idx] = updated;
          allAgents = next;
        }
      }
      // Refetch to pick up any fields not carried in the payload (color, handle).
      loadAgentRoster();
    });

    // Chat lifecycle → refresh thread list for current agent
    onWsEvent('nebo:chat_complete', () => refreshThreads());
    onWsEvent('nebo:chat_created', () => refreshThreads());
    onWsEvent('nebo:chat_title_updated', (data) => {
      // Patch title in place to avoid full reload
      const id = $page.params.agentId;
      if (!id || !data?.chatId) return;
      const threads = apiThreads[id];
      if (!threads) return;
      const thread = threads.find(t => t.id === data.chatId);
      if (thread && data.title) {
        thread.title = data.title;
        thread.name = data.title;
        apiThreads[id] = [...threads]; // trigger reactivity
      }
    });

    // Run/workflow updates → refresh runs + stats
    onWsEvent('nebo:run_update', () => refreshRuns());
    onWsEvent('nebo:workflow_update', () => refreshRuns());
    onWsEvent('nebo:workflow_run_started', () => refreshRuns());
    onWsEvent('nebo:workflow_run_completed', () => refreshRuns());
    onWsEvent('nebo:workflow_run_failed', () => refreshRuns());

    return () => {
      // Cleanup all WS event listeners
      for (const { event, handler } of wsHandlers) {
        window.removeEventListener(event, handler);
      }
      wsHandlers.length = 0;
    };
  });

  // Load agent-specific data when agentId changes
  $effect(() => {
    const id = $page.params.agentId;
    if (!id) return;
    loadAgentData(id);
  });

  async function loadAgentData(id: string) {
    threadsLoading[id] = true;
    apiRunsLoading[id] = true;
    try {
      const t0 = performance.now();
      const api = await import('$lib/api/nebo');
      console.log(`[nebo] import api: ${(performance.now() - t0).toFixed(0)}ms`);
      // Fire all requests in parallel but resolve threads first to unblock the UI
      const chatsPromise = api.listAgentChats(id).then(r => { console.log(`[nebo] chats: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] listAgentChats failed for', id, e); return null; });
      const runsPromise = api.listAgentRuns(id).then(r => { console.log(`[nebo] runs: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] listAgentRuns failed for', id, e); return null; });
      const statsPromise = api.agentStats(id).then(r => { console.log(`[nebo] stats: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] agentStats failed for', id, e); return null; });
      const agentPromise = api.getAgent(id).then(r => { console.log(`[nebo] agent: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] getAgent failed for', id, e); return null; });
      const workflowsPromise = api.listAgentWorkflows(id).then(r => { console.log(`[nebo] workflows: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] listAgentWorkflows failed for', id, e); return null; });

      // Unblock thread list as soon as chats arrive
      const chatsResp = await chatsPromise;
      if (chatsResp?.chats) apiThreads[id] = chatsResp.chats as EnrichedChat[];
      threadsLoading[id] = false;

      // Unblock runs list as soon as runs + stats arrive (don't wait for agent/workflows)
      const [runsResp, statsResp] = await Promise.all([runsPromise, statsPromise]);
      if (runsResp?.runs) {
        const rawRuns = runsResp.runs as RawRunRecord[];
        apiRuns[id] = mapRuns(rawRuns);
        apiRawRuns[id] = rawRuns;
        apiRunsTotal[id] = typeof runsResp.total === 'number' ? runsResp.total : rawRuns.length;
      }
      apiRunsLoading[id] = false;
      if (statsResp?.stats) {
        const s = statsResp.stats;
        const secs = s.avgDurationSecs ?? 0;
        const avgStr = secs > 0 ? (secs >= 60 ? `${Math.floor(secs / 60)}m ${Math.round(secs % 60)}s` : `${Math.round(secs)}s`) : '—';
        apiStats[id] = {
          totalRuns: s.totalRuns ?? 0,
          completed: s.completed ?? 0,
          failed: s.failed ?? 0,
          running: s.running ?? 0,
          avgDuration: avgStr,
          lastRunAt: s.lastRunAt ? new Date(s.lastRunAt * 1000).toLocaleString() : '—',
        };
      }

      // Agent + workflows settle in the background — don't block runs/stats UI
      const [agentResp, workflowsResp] = await Promise.all([agentPromise, workflowsPromise]);
      // Agent config (persona, model, skills, inputs)
      if (agentResp) {
        const ar = agentResp;
        apiSkills[id] = Array.isArray(ar.skills) ? ar.skills as string[] : [];
        const persona = typeof ar.persona === 'string' ? (ar.persona as string) : '';
        const agentMd = (ar.agent as Agent)?.agentMd || '';
        const soul = (ar.agent as Agent)?.soul || '';
        const rules = (ar.agent as Agent)?.rules || '';
        const model = typeof ar.model === 'string' ? ar.model : (ar.model as Record<string, unknown>)?.id as string ?? 'claude-sonnet-4-6';
        const inputs = Array.isArray(ar.inputFields) ? ar.inputFields as Record<string, unknown>[] : [];
        // Workflows from separate endpoint — merged below
        apiConfig[id] = { persona, agentMd, soul, rules, model, inputs, workflows: apiConfig[id]?.workflows ?? {} };

        // Auto-trigger onboarding if agent has unconfigured required inputs
        if (ar.needsSetup) {
          setupAgentName = ar.displayName || ar.agent?.name || '';
          setupAgentDesc = ar.agent?.description || '';
          setupInputFields = inputs;
          showSetupModal = true;
        } else {
          showSetupModal = false;
        }
      }
      // Workflows — backend returns a map keyed by binding name, not an array
      const wfMap = mapWorkflows(workflowsResp?.workflows);
      if (wfMap) {
        if (apiConfig[id]) {
          apiConfig[id] = { ...apiConfig[id], workflows: wfMap };
        } else {
          apiConfig[id] = { ...DEFAULT_CONFIG, workflows: wfMap };
        }
      }
    } catch (e) {
      console.error('[nebo] Failed to load agent data for', id, e);
    } finally {
      threadsLoading[id] = false;
      apiRunsLoading[id] = false;
    }
  }

  async function loadMoreRuns() {
    const id = $page.params.agentId;
    if (!id || apiRunsLoading[id]) return;
    const current = apiRuns[id]?.length ?? 0;
    const total = apiRunsTotal[id] ?? 0;
    if (current >= total) return;
    apiRunsLoading[id] = true;
    try {
      const { default: webapi } = await import('$lib/api/gocliRequest');
      const resp = await webapi.get<import('$lib/api/neboComponents').ListAgentRunsResponse>(
        `/api/v1/agents/${id}/runs`, { limit: 20, offset: current }
      );
      if (resp?.runs) {
        const newRaw = resp.runs as RawRunRecord[];
        apiRawRuns[id] = [...(apiRawRuns[id] || []), ...newRaw];
        apiRuns[id] = [...(apiRuns[id] || []), ...mapRuns(newRaw)];
        if (typeof resp.total === 'number') apiRunsTotal[id] = resp.total;
      }
    } catch { /* silent */ } finally {
      apiRunsLoading[id] = false;
    }
  }

  let agentStatuses = $state<Record<string, string>>({});

  function toggleAgentStatus(id: string, e?: MouseEvent) {
    if (e) { e.stopPropagation(); e.preventDefault(); }
    if (id === 'assistant') return; // Primary agent is always on
    const wasActive = agentStatuses[id] === 'online';
    agentStatuses[id] = wasActive ? 'paused' : 'online';
    // Fire API call
    import('$lib/api/nebo').then(api => {
      if (wasActive) {
        api.deactivateAgent(id);
      } else {
        api.activateAgent(id);
      }
    }).catch(() => {});
  }

  function agentStatus(id: string): string {
    return agentStatuses[id] ?? 'paused';
  }

  const sortedAgents = $derived.by(() => {
    const primary = allAgents.filter(a => a.id === 'assistant' && !a.isApp);
    const rest = allAgents.filter(a => a.id !== 'assistant' && !a.isApp).sort((a, b) => a.name.localeCompare(b.name));
    return [...primary, ...rest];
  });

  const sortedAppAgents = $derived.by(() => {
    return allAgents.filter(a => a.isApp).sort((a, b) => a.name.localeCompare(b.name));
  });

  const agentId = $derived($page.params.agentId ?? '');
  const agent = $derived(allAgents.find(a => a.id === agentId));
  const agentColor = $derived(agent ? AGENT_COLORS_MAP[agent.color] : null);
  const threads = $derived(agentId ? (apiThreads[agentId] || []) : []);
  const isThreadsLoading = $derived(agentId ? (threadsLoading[agentId] ?? true) : true);
  const runs = $derived(agentId ? (apiRuns[agentId] || []) : []);
  const runsTotal = $derived(agentId ? (apiRunsTotal[agentId] ?? 0) : 0);
  const hasMoreRuns = $derived(runs.length < runsTotal);
  const runsLoading = $derived(agentId ? (apiRunsLoading[agentId] ?? false) : false);
  const skills = $derived(agentId ? (apiSkills[agentId] || []) : []);
  const config = $derived(agentId ? (apiConfig[agentId] || DEFAULT_CONFIG) : DEFAULT_CONFIG);
  const workflowEntries = $derived(Object.entries(config.workflows));
  const workflowStats = $derived(agentId ? (apiStats[agentId] || { totalRuns: 0, completed: 0, failed: 0, running: 0, avgDuration: '—', lastRunAt: '—' }) : { totalRuns: 0, completed: 0, failed: 0, running: 0, avgDuration: '—', lastRunAt: '—' });
  const workflowRuns = $derived(agentId ? mapRawRunsToWFRuns(apiRawRuns[agentId] || []) : []);

  // Workflow canvas state
  let showCanvasModal = $state(false);
  let canvasFocusWorkflow = $state<string | null>(null);

  function triggerSummary(wf: WorkflowConfig): string {
    if (wf.trigger?.type === 'schedule') return wf.schedule || 'Scheduled';
    if (wf.trigger?.type === 'event') return `On ${wf.trigger.event || 'event'}`;
    if (wf.trigger?.type === 'watch') return `Watch: ${wf.trigger.event || wf.trigger.plugin || 'plugin'}`;
    if (wf.trigger?.type === 'heartbeat') return `Every ${wf.trigger.interval || '?'}`;
    return 'Manual trigger';
  }

  // Persist the full workflow map through the binding CRUD API, then sync
  // local state from what the server actually stored.
  async function persistWorkflows(wfs: Record<string, WorkflowConfig>): Promise<void> {
    const id = agentId;
    if (!id) return;
    const wfMap = await saveWorkflows(id, apiConfig[id]?.workflows ?? {}, wfs);
    if (wfMap) {
      apiConfig[id] = { ...(apiConfig[id] ?? DEFAULT_CONFIG), workflows: wfMap };
    }
  }

  async function toggleWorkflow(name: string): Promise<void> {
    const id = agentId;
    if (!id) return;
    const wf = apiConfig[id]?.workflows?.[name];
    if (wf) wf.isActive = wf.isActive === false; // optimistic flip
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.toggleAgentWorkflow(id, name) as { isActive?: boolean };
      if (wf && typeof resp?.isActive === 'boolean') wf.isActive = resp.isActive;
    } catch {
      if (wf) wf.isActive = wf.isActive === false; // revert
    }
  }

  // Clicking a workflow row opens the canvas builder focused on it — the one
  // editing surface. The "+ New workflow" button passes a freshly-built wf
  // that isn't in config yet; seed it into local state so the canvas sees it
  // (canvas Save persists it, same as any edit).
  function openWorkflow(name: string, wf: WorkflowConfig) {
    const id = agentId;
    if (id && !apiConfig[id]?.workflows?.[name]) {
      apiConfig[id] = {
        ...(apiConfig[id] ?? DEFAULT_CONFIG),
        workflows: { ...(apiConfig[id]?.workflows ?? {}), [name]: wf },
      };
    }
    canvasFocusWorkflow = name;
    showCanvasModal = true;
  }

  function openCanvas() {
    canvasFocusWorkflow = null;
    showCanvasModal = true;
  }


  function selectAgent(id: string) {
    const a = allAgents.find(ag => ag.id === id);
    goto(a?.isApp ? `/${id}/overview` : `/${id}/threads`);
  }

  function statusLabel(s: string) {
    if (s === 'online') return 'Online';
    if (s === 'running') return 'Running';
    if (s === 'paused') return 'Paused';
    return 'Idle';
  }

  // Agent context menu
  let ctxMenu = $state<{ x: number; y: number; agentId: string } | null>(null);

  // Delete confirmation
  let deleteTarget = $state<{ id: string; name: string } | null>(null);
  let deleting = $state(false);

  async function confirmDeleteAgent() {
    if (!deleteTarget || deleting) return;
    const targetId = deleteTarget.id;
    deleting = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteAgent(targetId);
      deleteTarget = null;
      deleting = false;
      loadAgentRoster();
      // Navigate away if we were viewing the deleted agent
      if (agentId === targetId) goto('/');
    } catch {
      deleting = false;
    }
  }

  function handleAgentContext(e: MouseEvent, aid: string) {
    e.preventDefault();
    ctxMenu = { x: e.clientX, y: e.clientY, agentId: aid };
  }

  function closeCtxMenu() {
    ctxMenu = null;
  }

  function ctxAction(action: string) {
    if (!ctxMenu) return;
    const id = ctxMenu.agentId;
    closeCtxMenu();

    if (action === 'toggle-status') {
      toggleAgentStatus(id);
    } else if (action === 'new-thread') {
      goto(`/${id}/threads`);
    } else if (action === 'copy-id') {
      navigator.clipboard.writeText(id);
    } else if (action === 'settings') {
      goto(`/${id}/settings/general`);
    } else if (action === 'open-app') {
      const a = allAgents.find(ag => ag.id === id);
      launchApp(id, a?.name || 'App');
    } else if (action === 'delete') {
      const a = allAgents.find(ag => ag.id === id);
      deleteTarget = { id, name: a?.name || 'this agent' };
    }
  }

  // Provide agent data to all children
  setContext('agentPage', {
    get agentId() { return agentId; },
    get agent() { return agent; },
    get agentColor() { return agentColor; },
    get threads() { return threads; },
    get isThreadsLoading() { return isThreadsLoading; },
    get agentsLoading() { return agentsLoading; },
    get runs() { return runs; },
    get runsTotal() { return runsTotal; },
    get hasMoreRuns() { return hasMoreRuns; },
    get runsLoading() { return runsLoading; },
    loadMoreRuns,
    get skills() { return skills; },
    get config() { return config; },
    get workflowEntries() { return workflowEntries; },
    get workflowStats() { return workflowStats; },
    get workflowRuns() { return workflowRuns; },
    get isApp() { return agent?.isApp ?? false; },
    get devMode() { return $devMode; },
    get agentStatuses() { return agentStatuses; },
    openWorkflow,
    openCanvas,
    triggerSummary,
    persistWorkflows,
    toggleWorkflow,
    toggleAgentStatus,
    agentStatus,
    refreshRuns,
    refreshThreads,
  });
</script>

<svelte:head><title>{agent?.name ?? 'Agent'} - Nebo</title></svelte:head>

<!-- Agent context menu -->
{#if ctxMenu}
  {@const ctxAgent = allAgents.find(a => a.id === ctxMenu?.agentId)}
  {@const ctxSt = agentStatus(ctxMenu.agentId)}
  <div class="fixed inset-0 z-50" onclick={closeCtxMenu} oncontextmenu={(e) => { e.preventDefault(); closeCtxMenu(); }} role="presentation"></div>
  <div
    class="fixed z-50 w-[180px] py-1 rounded-lg border border-base-300 bg-base-100 shadow-xl"
    style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;"
  >
    {#if ctxAgent?.isApp}
      <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors font-medium" onclick={() => ctxAction('open-app')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/50"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
        Open App
      </button>
      <div class="h-px bg-base-300 my-1"></div>
    {:else}
      {#if ctxMenu.agentId !== 'assistant'}
        <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('toggle-status')}>
          {#if ctxSt === 'paused'}
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" class="text-success"><polygon points="6,4 20,12 6,20"/></svg>
            Activate
          {:else}
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" class="text-base-content/50"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>
            Pause
          {/if}
        </button>
        <div class="h-px bg-base-300 my-1"></div>
      {/if}
      <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('new-thread')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/50"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
        New chat
      </button>
    {/if}
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('copy-id')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-base-content/50"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
      Copy Agent ID
    </button>
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('settings')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/50"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
      Settings
    </button>
    <div class="h-px bg-base-300 my-1"></div>
    {#if ctxAgent?.editable}
      <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-error/10 text-error transition-colors" onclick={() => ctxAction('delete')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
        Delete
      </button>
    {/if}
  </div>
{/if}

<!-- Delete agent confirmation modal -->
{#if deleteTarget}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation" onclick={() => { if (!deleting) deleteTarget = null; }} onkeydown={(e) => { if (e.key === 'Escape' && !deleting) deleteTarget = null; }}></div>
    <div class="relative w-full max-w-sm rounded-2xl bg-base-100 border border-error/30 shadow-2xl overflow-hidden">
      <div class="flex items-center gap-3 px-5 py-4 bg-error/5 border-b border-error/20">
        <div class="w-9 h-9 rounded-full bg-error/15 flex items-center justify-center">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-error"><path d="M10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
        </div>
        <div>
          <h3 class="text-sm font-bold">Delete {deleteTarget.name}?</h3>
          <p class="text-xs text-base-content/50">This cannot be undone</p>
        </div>
      </div>
      <div class="px-5 py-4">
        <p class="text-sm text-base-content/70">All threads, runs, and memory for this agent will be permanently removed.</p>
      </div>
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        <button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent" onclick={() => { deleteTarget = null; }} disabled={deleting}>Cancel</button>
        <button class="px-4 py-2 rounded-lg bg-error text-error-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none" onclick={confirmDeleteAgent} disabled={deleting}>
          {#if deleting}
            <span class="loading loading-spinner loading-xs"></span>
          {:else}
            Delete Agent
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Workflow editor modal -->
<!-- Workflow canvas builder — full-screen overlay -->
{#if showCanvasModal}
  <div class="fixed inset-0 z-[60] flex flex-col" data-modal-open>
    <div class="absolute inset-0 bg-black/40" role="presentation"></div>
    <div class="relative flex flex-col flex-1 m-4 rounded-2xl bg-base-100 border border-base-300 shadow-2xl z-10 overflow-hidden">
      <div class="flex items-center justify-between px-5 py-3 border-b border-base-content/10 shrink-0">
        <div class="flex items-center gap-3">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="8" y="14" width="7" height="7" rx="1"/><line x1="6.5" y1="10" x2="11.5" y2="14"/><line x1="17.5" y1="10" x2="11.5" y2="14"/></svg>
          <div>
            <div class="text-sm font-semibold">{agent?.name} &mdash; Workflow Builder</div>
            <div class="text-xs text-base-content/50">{workflowEntries.length} workflows &middot; {workflowEntries.reduce((sum, [, wf]) => sum + (wf.activities?.length ?? 0), 0)} activities</div>
          </div>
        </div>
        <button class="w-8 h-8 rounded-lg flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={() => showCanvasModal = false}>&times;</button>
      </div>
      <div class="flex-1 min-h-0">
        <WorkflowBuilder
          workflows={config.workflows}
          agentId={agentId}
          agentName={agent?.name ?? 'Agent'}
          focusWorkflow={canvasFocusWorkflow}
          onclose={() => { showCanvasModal = false; canvasFocusWorkflow = null; }}
          onsave={(wfs) => { showCanvasModal = false; canvasFocusWorkflow = null; persistWorkflows(wfs).catch((e) => console.error('[nebo] failed to save workflows', e)); }}
        />
      </div>
    </div>
  </div>
{/if}

<!-- Column 1: Agent roster -->
<div class="{$sidebarCollapsed ? 'w-12 min-w-12' : 'w-[260px] min-w-[260px]'} border-r border-base-300 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.08)] flex flex-col bg-base-200 shrink-0 transition-all duration-150">
  <div class="h-11 border-b border-base-300 flex items-center shrink-0 {$sidebarCollapsed ? 'justify-center' : 'px-3.5 justify-between'}">
    {#if !$sidebarCollapsed}
      <span class="text-sm font-semibold flex-1">Agents</span>
    {/if}
    <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none shrink-0" onclick={() => $sidebarCollapsed = !$sidebarCollapsed} title={$sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.2"/><line x1="5.5" y1="3" x2="5.5" y2="13" stroke="currentColor" stroke-width="1.2"/></svg>
    </button>
  </div>
  <div class="flex-1 overflow-y-auto overflow-x-hidden py-1">
    {#if agentsLoading && sortedAgents.length === 0}
      <div class="flex-1 flex items-center justify-center">
        <span class="loading loading-spinner loading-sm"></span>
      </div>
    {:else if $sidebarCollapsed}
      <div class="flex flex-col items-center gap-1 py-1">
        {#each sortedAgents as a}
          {@const st = agentStatus(a.id)}
          <div class="relative">
            <button
              class="w-8 h-8 rounded-field flex items-center justify-center font-mono text-sm font-semibold shrink-0 cursor-pointer transition-colors {agentId === a.id
                ? 'bg-primary text-primary-content border-none'
                : 'border border-base-300 bg-base-100'} {st === 'paused' ? 'opacity-50' : ''}"
              onclick={() => selectAgent(a.id)}
              oncontextmenu={(e) => handleAgentContext(e, a.id)}
              data-context-menu
              title="{a.name} — {statusLabel(st)}"
            >{a.initial}</button>
            <div class="absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full border-2 border-base-200 {st === 'running' ? 'bg-warning animate-pulse' : st === 'paused' ? 'bg-base-content/30' : 'bg-success'}"></div>
          </div>
        {/each}
        {#if sortedAppAgents.length > 0}
          <div class="h-px bg-base-content/10 mx-2 my-1.5"></div>
          {#each sortedAppAgents as a}
            <button
              class="w-8 h-8 rounded-field flex items-center justify-center shrink-0 cursor-pointer transition-colors {agentId === a.id
                ? 'bg-primary text-primary-content border-none'
                : 'border border-base-300 bg-base-100'}"
              onclick={() => selectAgent(a.id)}
              oncontextmenu={(e) => handleAgentContext(e, a.id)}
              data-context-menu
              title={a.name}
            >
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/></svg>
            </button>
          {/each}
        {/if}
      </div>
    {:else}
      {#each sortedAgents as a}
        {@const st = agentStatus(a.id)}
        <div
          class="group/agent flex items-center gap-2.5 py-2 px-2.5 mx-1.5 cursor-pointer transition-colors text-left {agentId === a.id
            ? 'rounded-box border border-base-300 bg-base-100 shadow-sm'
            : 'rounded-box border border-transparent hover:bg-base-100/70'}"
        >
          <button
            class="flex items-center gap-2.5 flex-1 min-w-0 bg-transparent border-none cursor-pointer p-0 text-left"
            onclick={() => selectAgent(a.id)}
            oncontextmenu={(e) => handleAgentContext(e, a.id)}
            data-context-menu
          >
            <div class="w-8 h-8 rounded-field flex items-center justify-center font-mono text-sm font-semibold shrink-0 {agentId === a.id
              ? 'bg-primary text-primary-content'
              : 'border border-base-300 bg-base-100'}">{a.initial}</div>
            <div class="flex-1 min-w-0">
              <div class="text-sm font-medium truncate">{a.name}</div>
              <div class="text-xs text-base-content/70 truncate">{a.role}</div>
            </div>
          </button>
          <!-- Status toggle (not for primary agent) -->
          {#if a.id !== 'assistant'}
            <div class="relative shrink-0">
              <input type="checkbox" class="toggle toggle-xs {st === 'running' ? 'toggle-warning' : 'toggle-success'}" checked={st !== 'paused'}
                onchange={(e) => { e.stopPropagation(); toggleAgentStatus(a.id); }}
              />
              {#if st === 'running'}
                <div class="absolute top-0.5 right-0.5 w-1.5 h-1.5 rounded-full bg-warning animate-pulse pointer-events-none"></div>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
      {#if sortedAppAgents.length > 0}
        <div class="px-3.5 pt-3 pb-1">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Agent Apps</div>
        </div>
        {#each sortedAppAgents as a}
          {@const st = agentStatus(a.id)}
          <div
            class="group/agent flex items-center gap-2.5 py-2 px-2.5 mx-1.5 cursor-pointer transition-colors text-left {agentId === a.id
              ? 'rounded-box border border-base-300 bg-base-100 shadow-sm'
              : 'rounded-box border border-transparent hover:bg-base-100/70'}"
          >
            <button
              class="flex items-center gap-2.5 flex-1 min-w-0 bg-transparent border-none cursor-pointer p-0 text-left"
              onclick={() => selectAgent(a.id)}
              oncontextmenu={(e) => handleAgentContext(e, a.id)}
              data-context-menu
            >
              <div class="w-8 h-8 rounded-field flex items-center justify-center shrink-0 {agentId === a.id
                ? 'bg-primary text-primary-content'
                : 'border border-base-300 bg-base-100'}">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/></svg>
              </div>
              <div class="flex-1 min-w-0">
                <div class="text-sm font-medium truncate">{a.name}</div>
                <div class="text-xs text-base-content/70 truncate">{a.role}</div>
              </div>
            </button>
            <div class="relative shrink-0">
              <input type="checkbox" class="toggle toggle-xs {st === 'running' ? 'toggle-warning' : 'toggle-success'}" checked={st !== 'paused'}
                onchange={(e) => { e.stopPropagation(); toggleAgentStatus(a.id); }}
              />
              {#if st === 'running'}
                <div class="absolute top-0.5 right-0.5 w-1.5 h-1.5 rounded-full bg-warning animate-pulse pointer-events-none"></div>
              {/if}
            </div>
          </div>
        {/each}
      {/if}
    {/if}
  </div>
  <UserMenu collapsed={$sidebarCollapsed} />
</div>

<!-- Columns 2+3: rendered by child routes -->
{@render children()}

<!-- Configure-existing wizard, auto-opened when an installed agent still needs setup. -->
{#if showSetupModal}
  <InstallFlowModal
    mode="configure"
    bind:show={showSetupModal}
    existingAgentId={$page.params.agentId}
    agentName={setupAgentName}
    agentDescription={setupAgentDesc}
    seedInputs={setupInputFields}
    oncomplete={(id) => { showSetupModal = false; const target = id ?? $page.params.agentId; if (target) loadAgentData(target); }}
  />
{/if}

<!-- Paste-a-code install door (WS-driven; opens itself on nebo:code_processing). -->
<InstallFlowModal bind:show={showInstallModal} mode="code" />
