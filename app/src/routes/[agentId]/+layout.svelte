<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { setContext, onMount } from 'svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import WorkflowBuilder from '$lib/components/workflow/WorkflowBuilder.svelte';
  import AgentSetupModal from '$lib/components/agent-setup/AgentSetupModal.svelte';
  import CodeInstallModal from '$lib/components/chat/CodeInstallModal.svelte';
  import { launchApp } from '$lib/apps/launcher.js';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar.js';
  const sidebarCollapsed = sidebarCollapsedFor('agents');
  import { devMode } from '$lib/stores/devmode.js';
  import { ACTIVITY_TYPES, getActivityType, createTypedActivity, isBranchingType, type ActivityType } from '$lib/utils/workflowTypes';
  import { generateLinearConnections, removeConnection, type WorkflowConnection } from '$lib/utils/workflowLayout';
  import type { AgentDisplay, EnrichedChat, AgentRun, WorkflowStatsLocal, WorkflowConfig } from '$lib/types/agentPage';
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
      const wfData = workflowsResp?.workflows;
      if (wfData && typeof wfData === 'object' && !Array.isArray(wfData)) {
        const wfMap: Record<string, WorkflowConfig> = {};
        for (const [name, raw] of Object.entries(wfData)) {
          if (!name) continue;
          const wf = raw as Record<string, unknown>;
          const trigger = (wf.trigger || {}) as Record<string, unknown>;
          wfMap[name] = {
            trigger: { type: String(trigger.type || 'manual'), event: trigger.event as string, schedule: trigger.schedule as string || trigger.cron as string },
            schedule: (trigger.schedule || trigger.cron) as string,
            activities: Array.isArray(wf.activities) ? wf.activities as WorkflowConfig['activities'] : [],
            connections: Array.isArray(wf.connections) ? wf.connections : undefined,
            isActive: wf.isActive === true,
            description: typeof wf.description === 'string' ? wf.description : undefined,
            lastFired: typeof wf.lastFired === 'string' ? wf.lastFired : undefined,
            emit: typeof wf.emit === 'string' ? wf.emit : undefined,
            source: typeof wf.source === 'string' ? wf.source : undefined,
          };
        }
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

  // Workflow modal state
  let editingWorkflow = $state<{ name: string; wf: WorkflowConfig } | null>(null);
  let expandedActivities = $state<Record<number, boolean>>({});
  let showCanvasModal = $state(false);
  let deleteConfirmIdx = $state<number | null>(null);
  let skillSearchIdx = $state<number | null>(null);
  let skillSearchQuery = $state('');

  function triggerSummary(wf: WorkflowConfig): string {
    if (wf.trigger?.type === 'schedule') return wf.schedule || 'Scheduled';
    if (wf.trigger?.type === 'event') return `On ${wf.trigger.event || 'event'}`;
    return 'Manual trigger';
  }

  function openWorkflow(name: string, wf: WorkflowConfig) {
    editingWorkflow = { name, wf };
    expandedActivities = {};
  }

  function toggleActivity(idx: number) {
    expandedActivities[idx] = !expandedActivities[idx];
  }

  function openCanvas() {
    showCanvasModal = true;
  }

  // Activity type picker state
  let showTypePicker = $state(false);
  let typePickerSearch = $state('');
  const filteredTypes = $derived.by(() => {
    const q = typePickerSearch.toLowerCase().trim();
    const entries = Object.values(ACTIVITY_TYPES);
    if (!q) return entries;
    return entries.filter(t => t.label.toLowerCase().includes(q) || t.description.toLowerCase().includes(q));
  });

  function addActivity() {
    showTypePicker = true;
    typePickerSearch = '';
  }

  function addTypedActivity(actType: ActivityType) {
    if (!editingWorkflow) return;
    const activities = editingWorkflow.wf.activities ?? [];
    const typeDef = ACTIVITY_TYPES[actType];
    const id = typeDef.label.toLowerCase().replace(/\s+/g, '-') + '-' + Date.now().toString(36);
    const params: Record<string, any> = {};
    for (const p of typeDef.parameters) {
      if (p.default !== undefined) params[p.key] = p.default;
    }
    const newActivity = {
      id,
      type: actType,
      intent: typeDef.description,
      skills: [...typeDef.defaultSkills],
      steps: [...typeDef.defaultSteps],
      params,
    };
    editingWorkflow = {
      ...editingWorkflow,
      wf: {
        ...editingWorkflow.wf,
        activities: [...activities, newActivity],
      },
    };
    expandedActivities[activities.length] = true;
    showTypePicker = false;
    typePickerSearch = '';
  }

  function changeActivityType(activityIdx: number, newType: ActivityType) {
    if (!editingWorkflow) return;
    const typeDef = ACTIVITY_TYPES[newType];
    const activity = editingWorkflow.wf.activities?.[activityIdx];
    if (!activity) return;
    const params: Record<string, any> = {};
    for (const p of typeDef.parameters) {
      if (p.default !== undefined) params[p.key] = p.default;
    }
    updateActivity(activityIdx, {
      type: newType,
      skills: activity.skills?.length ? activity.skills : [...typeDef.defaultSkills],
      steps: activity.steps?.length ? activity.steps : [...typeDef.defaultSteps],
      params: { ...(activity.params ?? {}), ...params },
    });
  }

  function duplicateActivity(activityIdx: number) {
    if (!editingWorkflow) return;
    const activities = editingWorkflow.wf.activities ?? [];
    const original = activities[activityIdx];
    if (!original) return;
    const dupe = {
      ...JSON.parse(JSON.stringify(original)),
      id: `${original.id}-copy-${Date.now().toString(36)}`,
    };
    const newActivities = [...activities];
    newActivities.splice(activityIdx + 1, 0, dupe);

    // Also duplicate connections if they exist
    let newConnections = editingWorkflow.wf.connections ? [...editingWorkflow.wf.connections] as WorkflowConnection[] : undefined;
    if (newConnections) {
      const outgoing = newConnections.filter((c) => c.from === original.id);
      if (outgoing.length > 0) {
        const firstTarget = outgoing[0].to;
        const firstLabel = outgoing[0].label;
        const idx = newConnections.indexOf(outgoing[0]);
        newConnections.splice(idx, 1, { from: original.id, to: dupe.id, ...(firstLabel ? { label: firstLabel } : {}) });
        newConnections.push({ from: dupe.id, to: firstTarget });
      } else {
        newConnections.push({ from: original.id, to: dupe.id });
      }
    }

    editingWorkflow = {
      ...editingWorkflow,
      wf: {
        ...editingWorkflow.wf,
        activities: newActivities,
        ...(newConnections ? { connections: newConnections } : {}),
      },
    };
    expandedActivities[activityIdx + 1] = true;
  }

  function moveActivity(activityIdx: number, direction: -1 | 1) {
    if (!editingWorkflow) return;
    const activities = [...(editingWorkflow.wf.activities ?? [])];
    const targetIdx = activityIdx + direction;
    if (targetIdx < 0 || targetIdx >= activities.length) return;
    [activities[activityIdx], activities[targetIdx]] = [activities[targetIdx], activities[activityIdx]];

    // Update expanded state
    const newExpanded: Record<number, boolean> = {};
    for (const [k, v] of Object.entries(expandedActivities)) {
      const ki = Number(k);
      if (ki === activityIdx) newExpanded[targetIdx] = v;
      else if (ki === targetIdx) newExpanded[activityIdx] = v;
      else newExpanded[ki] = v;
    }
    expandedActivities = newExpanded;

    editingWorkflow = {
      ...editingWorkflow,
      wf: { ...editingWorkflow.wf, activities },
    };
  }

  // Connection management
  function addNewConnection(from: string, to: string, label?: string) {
    if (!editingWorkflow) return;
    let conns: WorkflowConnection[] = editingWorkflow.wf.connections
      ? [...editingWorkflow.wf.connections] as WorkflowConnection[]
      : generateLinearConnections(editingWorkflow.wf.activities ?? [], editingWorkflow.wf.emit);
    const exists = conns.some((c) => c.from === from && c.to === to);
    if (exists) return;
    conns.push({ from, to, ...(label ? { label } : {}) });
    editingWorkflow = {
      ...editingWorkflow,
      wf: { ...editingWorkflow.wf, connections: conns },
    };
  }

  function removeWorkflowConnection(from: string, to: string) {
    if (!editingWorkflow || !editingWorkflow.wf.connections) return;
    editingWorkflow = {
      ...editingWorkflow,
      wf: {
        ...editingWorkflow.wf,
        connections: removeConnection(editingWorkflow.wf.connections, from, to),
      },
    };
  }

  // Connection editor state
  let showAddConnection = $state(false);
  let newConnFrom = $state('');
  let newConnTo = $state('');
  let newConnLabel = $state('');

  const allNodeIds = $derived.by(() => {
    if (!editingWorkflow) return [];
    const ids = ['__trigger__'];
    for (const a of editingWorkflow.wf.activities ?? []) ids.push(a.id);
    if (editingWorkflow.wf.emit) ids.push('__emit__');
    return ids;
  });

  function toKebab(value: string): string {
    return value.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '').replace(/-+/g, '-');
  }

  function emitNameFromWorkflow(name: string): string {
    return toKebab(name).replace(/-/g, '.') + '.completed';
  }

  function toggleEmit(checked: boolean) {
    if (!editingWorkflow) return;
    editingWorkflow = {
      ...editingWorkflow,
      wf: {
        ...editingWorkflow.wf,
        emit: checked ? emitNameFromWorkflow(editingWorkflow.name) : undefined,
      },
    };
  }

  function updateActivity(activityIdx: number, patch: Record<string, any>) {
    if (!editingWorkflow) return;
    const activities = [...(editingWorkflow.wf.activities ?? [])];
    activities[activityIdx] = { ...activities[activityIdx], ...patch };
    editingWorkflow = {
      ...editingWorkflow,
      wf: { ...editingWorkflow.wf, activities },
    };
  }

  function addStep(activityIdx: number) {
    if (!editingWorkflow) return;
    const activity = editingWorkflow.wf.activities?.[activityIdx];
    if (!activity) return;
    updateActivity(activityIdx, { steps: [...(activity.steps ?? []), ''] });
  }

  function addSkill(activityIdx: number, skill: string) {
    if (!editingWorkflow || !skill.trim()) return;
    const activity = editingWorkflow.wf.activities?.[activityIdx];
    if (!activity) return;
    const existing = activity.skills ?? [];
    if (existing.includes(skill.trim())) return;
    updateActivity(activityIdx, { skills: [...existing, skill.trim()] });
  }

  function removeSkill(activityIdx: number, skill: string) {
    if (!editingWorkflow) return;
    const activity = editingWorkflow.wf.activities?.[activityIdx];
    if (!activity) return;
    updateActivity(activityIdx, { skills: (activity.skills ?? []).filter((s: string) => s !== skill) });
  }

  function removeActivity(activityIdx: number) {
    if (!editingWorkflow) return;
    const activities = (editingWorkflow.wf.activities ?? []).filter((_: unknown, i: number) => i !== activityIdx);
    editingWorkflow = {
      ...editingWorkflow,
      wf: { ...editingWorkflow.wf, activities },
    };
    deleteConfirmIdx = null;
    // Clean up expanded state
    const newExpanded: Record<number, boolean> = {};
    for (const [k, v] of Object.entries(expandedActivities)) {
      const ki = Number(k);
      if (ki < activityIdx) newExpanded[ki] = v;
      else if (ki > activityIdx) newExpanded[ki - 1] = v;
    }
    expandedActivities = newExpanded;
  }

  function openSkillSearch(activityIdx: number) {
    skillSearchIdx = activityIdx;
    skillSearchQuery = '';
  }

  function closeSkillSearch() {
    skillSearchIdx = null;
    skillSearchQuery = '';
  }

  const filteredSkills = $derived.by(() => {
    if (skillSearchIdx === null || !editingWorkflow) return [];
    const activity = editingWorkflow.wf.activities?.[skillSearchIdx];
    const existing = new Set(activity?.skills ?? []);
    const query = skillSearchQuery.toLowerCase();
    return skills.filter((s: string) => !existing.has(s) && s.toLowerCase().includes(query));
  });

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
        New Thread
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
{#if editingWorkflow}
  {@const ew = editingWorkflow}
  {@const purchased = ew.wf.source === 'marketplace'}
  <div class="fixed inset-0 z-50 flex items-center justify-center" data-modal-open>
    <div class="absolute inset-0 bg-black/30" role="presentation"></div>
    <div class="relative bg-base-100 rounded-xl border border-base-300 shadow-xl w-[620px] max-h-[80vh] flex flex-col z-10">
      <!-- Header -->
      <div class="flex items-center justify-between px-5 py-3.5 border-b border-base-300 shrink-0">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <span class="text-sm font-semibold">{ew.name || 'Untitled Workflow'}</span>
            {#if purchased}
              <span class="py-0 px-1.5 rounded bg-base-200 text-xs font-mono">Marketplace</span>
            {/if}
            {#if ew.wf.isActive === false}
              <span class="py-0 px-1.5 rounded bg-base-200 text-xs text-base-content/50">Paused</span>
            {/if}
          </div>
          <div class="text-xs text-base-content/70">{agent?.name} &middot; {ew.wf.activities?.length ?? 0} activities</div>
        </div>
        <div class="flex items-center gap-2 shrink-0">
          {#if !purchased}
            <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={ew.wf.isActive !== false} role="switch" title="Enable/disable workflow" />
          {/if}
          <button class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={() => editingWorkflow = null}>&times;</button>
        </div>
      </div>

      {#if purchased}
        <div class="flex items-center gap-2 px-5 py-2 bg-base-200 border-b border-base-300 text-xs">
          <span>&#128274;</span>
          <span>Installed from Marketplace &mdash; read-only.</span>
          <button class="ml-auto py-0.5 px-2 rounded border border-base-300 bg-base-100 text-xs font-medium cursor-pointer hover:bg-base-100 transition-colors">Duplicate as custom</button>
        </div>
      {/if}

      <div class="flex-1 overflow-y-auto p-5">
        <!-- Name -->
        <div class="mb-5">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Name</div>
          {#if purchased}
            <div class="text-sm">{ew.name}</div>
          {:else}
            <input type="text" value={ew.name} placeholder="e.g. morning-scan"
              oninput={(e) => {
                const raw = e.currentTarget.value;
                const newName = toKebab(raw);
                e.currentTarget.value = newName;
                const hadEmit = !!editingWorkflow!.wf.emit;
                editingWorkflow = {
                  ...editingWorkflow!,
                  name: newName,
                  wf: {
                    ...editingWorkflow!.wf,
                    emit: hadEmit ? emitNameFromWorkflow(newName) : undefined,
                  },
                };
              }}
              class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm outline-none font-mono bg-base-100" />
          {/if}
        </div>

        <!-- Trigger -->
        <div class="mb-5">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Trigger</div>
          {#if purchased}
            <div class="py-1.5 px-3 rounded-md border border-base-300 bg-base-200 text-sm inline-flex items-center gap-1.5">
              {#if ew.wf.trigger?.type === 'schedule'}&#8635;{:else if ew.wf.trigger?.type === 'event'}&#9889;{:else}&#9654;{/if}
              <span class="capitalize">{ew.wf.trigger?.type ?? 'manual'}</span>
            </div>
          {:else}
            <div class="grid grid-cols-4 gap-1.5">
              {#each [
                { type: 'schedule', icon: '&#8635;', label: 'Schedule', desc: 'Run at set times' },
                { type: 'heartbeat', icon: '&#10084;', label: 'Heartbeat', desc: 'Run on interval' },
                { type: 'event', icon: '&#9889;', label: 'Event', desc: 'React to events' },
                { type: 'manual', icon: '&#9654;', label: 'Manual', desc: 'Run on demand' },
              ] as tt}
                <button class="flex flex-col items-center gap-0.5 py-2 px-2 rounded-lg text-center cursor-pointer border transition-colors {ew.wf.trigger?.type === tt.type ? 'bg-primary/5 border-primary text-primary' : 'bg-base-100 border-base-300 hover:bg-base-200'}">
                  <span class="text-base">{@html tt.icon}</span>
                  <span class="text-xs font-medium">{tt.label}</span>
                </button>
              {/each}
            </div>
          {/if}

          <!-- Trigger config by type -->
          {#if ew.wf.trigger?.type === 'schedule' && !purchased}
            <div class="mt-3 flex flex-col gap-2">
              <input type="text" value={ew.wf.schedule ?? ''} placeholder="e.g. 8:00 AM daily"
                class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm outline-none font-mono bg-base-100" />
              <div class="flex gap-1.5">
                {#each ['Weekdays', 'Weekends', 'Daily', 'Custom'] as preset}
                  <button class="py-1 px-2.5 rounded-md text-xs cursor-pointer border border-base-300 bg-base-100 hover:bg-base-200 transition-colors">{preset}</button>
                {/each}
              </div>
            </div>
          {:else if ew.wf.trigger?.type === 'schedule' && purchased}
            <div class="mt-2 py-1.5 px-2.5 rounded-md border border-base-300 bg-base-200 text-sm font-mono">{ew.wf.schedule ?? ''}</div>
          {:else if ew.wf.trigger?.type === 'heartbeat' && !purchased}
            <div class="mt-3 flex flex-col gap-2">
              <select class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-mono">
                <option value="5m">Every 5 minutes</option>
                <option value="10m">Every 10 minutes</option>
                <option value="15m">Every 15 minutes</option>
                <option value="30m" selected>Every 30 minutes</option>
                <option value="1h">Every hour</option>
                <option value="2h">Every 2 hours</option>
                <option value="4h">Every 4 hours</option>
                <option value="8h">Every 8 hours</option>
              </select>
              <div class="text-xs text-base-content/50">Optional: restrict to specific hours (e.g. 9 AM - 6 PM)</div>
            </div>
          {:else if ew.wf.trigger?.type === 'event'}
            <div class="mt-3">
              <input type="text" value={ew.wf.trigger?.event ?? ''} placeholder="e.g. GitHub PR opened, email.received" disabled={purchased}
                class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm outline-none font-mono {purchased ? 'bg-base-200' : 'bg-base-100'}" />
              <div class="text-xs text-base-content/50 mt-1">Comma-separated event names</div>
            </div>
          {/if}
        </div>

        <!-- Description -->
        <div class="mb-5">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Description</div>
          {#if purchased}
            <div class="text-sm leading-relaxed">{ew.wf.description ?? ''}</div>
          {:else}
            <textarea rows="2"
              class="w-full py-[7px] px-2.5 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-body leading-relaxed">{ew.wf.description ?? ''}</textarea>
          {/if}
        </div>

        <!-- Emit event -->
        {#if !purchased}
          <div class="mb-5">
            <div class="flex items-center justify-between mb-1.5">
              <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">Emit Event</div>
              <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={!!ew.wf.emit} onchange={(e) => toggleEmit(e.currentTarget.checked)} role="switch" title="Emit event on completion" />
            </div>
            {#if ew.wf.emit}
              <div class="py-1.5 px-2.5 rounded-md border border-base-300 bg-base-200/50 text-sm font-mono text-base-content/70">{ew.wf.emit}</div>
              <div class="text-xs text-base-content/50 mt-1">Other workflows can listen for this event as a trigger.</div>
            {:else}
              <div class="text-xs text-base-content/50">Enable to announce an event when this workflow completes.</div>
            {/if}
          </div>
        {:else if ew.wf.emit}
          <div class="mb-5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">Emits</div>
            <div class="py-1 px-2 rounded bg-accent/10 text-xs text-accent font-mono inline-block">&#8594; {ew.wf.emit}</div>
          </div>
        {/if}

        <!-- Activities sequence -->
        <div>
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Activities</div>
          <div class="flex flex-col gap-0">
            {#each ew.wf.activities ?? [] as activity, idx}
              {@const typeDef = getActivityType(activity.type)}
              <!-- Connector line -->
              {#if idx > 0}
                <div class="flex justify-center py-1">
                  <div class="w-px h-4 bg-base-300"></div>
                </div>
              {/if}

              <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
                <!-- Activity header -->
                <button
                  class="w-full flex items-center gap-2.5 px-3.5 py-2.5 text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors"
                  onclick={() => toggleActivity(idx)}
                >
                  <div class="w-6 h-6 rounded-md bg-base-200 border {typeDef.accentClass} flex items-center justify-center text-sm shrink-0">{typeDef.icon}</div>
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-1.5">
                      <span class="text-sm font-medium">{activity.id}</span>
                      <span class="py-0 px-1 rounded bg-base-200 text-xs text-base-content/50 font-mono">{typeDef.label}</span>
                      {#if isBranchingType(activity.type)}
                        <span class="py-0 px-1 rounded bg-accent/10 text-xs text-accent font-mono">branching</span>
                      {/if}
                    </div>
                    <div class="text-xs text-base-content/70 truncate">{activity.intent}</div>
                  </div>
                  {#if !purchased}
                    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
                    <div class="flex items-center gap-0.5 shrink-0" role="group" onclick={(e) => e.stopPropagation()} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') e.stopPropagation(); }}>
                      <span class="w-5 h-5 rounded flex items-center justify-center text-xs text-base-content/40 hover:text-base-content hover:bg-base-200 cursor-pointer bg-transparent border-none disabled:opacity-30 disabled:cursor-not-allowed {idx === 0 ? 'opacity-30 cursor-not-allowed' : ''}" role="button" tabindex="0" aria-disabled={idx === 0} onclick={() => { if (idx !== 0) moveActivity(idx, -1); }} onkeydown={(e) => { if ((e.key === 'Enter' || e.key === ' ') && idx !== 0) { e.preventDefault(); moveActivity(idx, -1); } }} title="Move up">&#9650;</span>
                      <span class="w-5 h-5 rounded flex items-center justify-center text-xs text-base-content/40 hover:text-base-content hover:bg-base-200 cursor-pointer bg-transparent border-none disabled:opacity-30 disabled:cursor-not-allowed {idx === (ew.wf.activities?.length ?? 0) - 1 ? 'opacity-30 cursor-not-allowed' : ''}" role="button" tabindex="0" aria-disabled={idx === (ew.wf.activities?.length ?? 0) - 1} onclick={() => { if (idx !== (ew.wf.activities?.length ?? 0) - 1) moveActivity(idx, 1); }} onkeydown={(e) => { if ((e.key === 'Enter' || e.key === ' ') && idx !== (ew.wf.activities?.length ?? 0) - 1) { e.preventDefault(); moveActivity(idx, 1); } }} title="Move down">&#9660;</span>
                    </div>
                  {/if}
                  <span class="text-sm transition-transform shrink-0 {expandedActivities[idx] ? 'rotate-90' : ''}">&rsaquo;</span>
                </button>

                <!-- Expanded detail -->
                {#if expandedActivities[idx]}
                  <div class="px-3.5 pb-3.5 border-t border-base-300">
                    <!-- Type selector -->
                    {#if !purchased}
                      <div class="mt-3">
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Type</div>
                        <select
                          class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body"
                          value={activity.type || 'custom'}
                          onchange={(e) => changeActivityType(idx, e.currentTarget.value as ActivityType)}
                        >
                          {#each Object.values(ACTIVITY_TYPES) as t}
                            <option value={t.type}>{t.icon} {t.label} — {t.description}</option>
                          {/each}
                        </select>
                      </div>
                    {:else}
                      <div class="mt-3">
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Type</div>
                        <div class="text-sm inline-flex items-center gap-1.5">
                          <span>{typeDef.icon}</span>
                          <span>{typeDef.label}</span>
                        </div>
                      </div>
                    {/if}
                    <!-- Name -->
                    <div class="mt-3">
                      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Name</div>
                      {#if purchased}
                        <div class="text-sm font-mono">{activity.id}</div>
                      {:else}
                        <input type="text" value={activity.id} placeholder="e.g. scan-sources"
                          oninput={(e) => { e.currentTarget.value = toKebab(e.currentTarget.value); updateActivity(idx, { id: toKebab(e.currentTarget.value) }); }}
                          class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-mono" />
                      {/if}
                    </div>
                    <!-- Intent -->
                    <div class="mt-3">
                      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Intent</div>
                      {#if purchased}
                        <div class="text-sm">{activity.intent}</div>
                      {:else}
                        <input type="text" value={activity.intent} placeholder="What should this step accomplish?"
                          oninput={(e) => updateActivity(idx, { intent: e.currentTarget.value })}
                          class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body" />
                      {/if}
                    </div>
                    <!-- Type-specific parameters -->
                    {#if typeDef.parameters.length > 0}
                      <div class="mt-3">
                        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Parameters</div>
                        <div class="flex flex-col gap-2.5">
                          {#each typeDef.parameters as param}
                            <div>
                              <div class="text-xs text-base-content/70 mb-0.5">{param.label}</div>
                              {#if param.description}
                                <div class="text-xs text-base-content/40 mb-1">{param.description}</div>
                              {/if}
                              {#if purchased}
                                <div class="text-sm font-mono">{activity.params?.[param.key] ?? param.default ?? '—'}</div>
                              {:else if param.type === 'select'}
                                <select
                                  class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-body"
                                  value={activity.params?.[param.key] ?? param.default ?? ''}
                                  onchange={(e) => {
                                    const params = { ...(activity.params ?? {}), [param.key]: e.currentTarget.value };
                                    updateActivity(idx, { params });
                                  }}
                                >
                                  {#each param.options ?? [] as opt}
                                    <option value={opt.value}>{opt.label}</option>
                                  {/each}
                                </select>
                              {:else if param.type === 'textarea'}
                                <textarea rows="3"
                                  class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none resize-y font-mono leading-relaxed"
                                  placeholder={param.placeholder ?? ''}
                                  oninput={(e) => {
                                    const params = { ...(activity.params ?? {}), [param.key]: e.currentTarget.value };
                                    updateActivity(idx, { params });
                                  }}
                                >{activity.params?.[param.key] ?? param.default ?? ''}</textarea>
                              {:else if param.type === 'number'}
                                <input type="number"
                                  class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-mono"
                                  value={activity.params?.[param.key] ?? param.default ?? ''}
                                  placeholder={param.placeholder ?? ''}
                                  oninput={(e) => {
                                    const params = { ...(activity.params ?? {}), [param.key]: Number(e.currentTarget.value) };
                                    updateActivity(idx, { params });
                                  }}
                                />
                              {:else if param.type === 'toggle'}
                                <input type="checkbox" class="toggle toggle-sm toggle-primary"
                                  checked={!!(activity.params?.[param.key] ?? param.default ?? false)}
                                  onchange={(e) => {
                                    const params = { ...(activity.params ?? {}), [param.key]: e.currentTarget.checked };
                                    updateActivity(idx, { params });
                                  }}
                                  role="switch"
                                />
                              {:else}
                                <input type="text"
                                  class="w-full py-[6px] px-2 rounded-md border border-base-300 text-sm bg-base-100 outline-none font-mono"
                                  value={activity.params?.[param.key] ?? param.default ?? ''}
                                  placeholder={param.placeholder ?? ''}
                                  oninput={(e) => {
                                    const params = { ...(activity.params ?? {}), [param.key]: e.currentTarget.value };
                                    updateActivity(idx, { params });
                                  }}
                                />
                              {/if}
                            </div>
                          {/each}
                        </div>
                      </div>
                    {/if}
                    <!-- Skills -->
                    <div class="mt-3">
                      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Skills</div>
                      <div class="flex flex-wrap gap-1.5">
                        {#each activity.skills ?? [] as skill}
                          <span class="py-0.5 px-2 rounded bg-base-200 font-mono text-xs inline-flex items-center gap-1">
                            {skill}
                            {#if !purchased}
                              <button class="hover:text-error cursor-pointer bg-transparent border-none text-xs p-0 leading-none" onclick={() => removeSkill(idx, skill)}>&times;</button>
                            {/if}
                          </span>
                        {/each}
                        {#if !purchased}
                          <div class="relative">
                            <input type="text" placeholder="+ add skill"
                              onfocus={() => openSkillSearch(idx)}
                              oninput={(e) => { skillSearchQuery = e.currentTarget.value; }}
                              onkeydown={(e) => {
                                if (e.key === 'Escape') { closeSkillSearch(); e.currentTarget.blur(); }
                                else if (e.key === 'Enter' && filteredSkills.length > 0) { addSkill(idx, filteredSkills[0]); e.currentTarget.value = ''; skillSearchQuery = ''; }
                                else if (e.key === 'Enter' && e.currentTarget.value.trim()) { addSkill(idx, e.currentTarget.value.trim()); e.currentTarget.value = ''; skillSearchQuery = ''; }
                              }}
                              onblur={() => { setTimeout(() => closeSkillSearch(), 150); }}
                              class="py-0.5 px-2 rounded border border-dashed border-base-300 text-xs bg-transparent outline-none font-mono w-28 placeholder:text-primary" />
                            {#if skillSearchIdx === idx && (filteredSkills.length > 0 || skillSearchQuery)}
                              <div class="absolute top-full left-0 mt-1 w-56 max-h-48 flex flex-col rounded-lg border border-base-300 bg-base-100 shadow-lg z-20">
                                <div class="flex-1 overflow-y-auto">
                                  {#if filteredSkills.length > 0}
                                    {#each filteredSkills.slice(0, 20) as s}
                                      <button
                                        class="w-full text-left px-2.5 py-1.5 text-xs font-mono cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors"
                                        onmousedown={() => { addSkill(idx, s); skillSearchQuery = ''; }}
                                      >{s}</button>
                                    {/each}
                                  {:else}
                                    <div class="px-2.5 py-1.5 text-xs text-base-content/50">No matching skills</div>
                                  {/if}
                                </div>
                                {#if filteredSkills.length > 20}
                                  <div class="px-2.5 py-1.5 text-xs text-base-content/50 border-t border-base-300 shrink-0">+{filteredSkills.length - 20} more — type to filter</div>
                                {/if}
                              </div>
                            {/if}
                          </div>
                        {/if}
                      </div>
                    </div>
                    <!-- Steps -->
                    <div class="mt-3">
                      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">Steps</div>
                      <div class="flex flex-col gap-1">
                        {#each activity.steps ?? [] as step, stepIdx}
                          <div class="flex items-start gap-2 group">
                            <span class="font-mono text-xs mt-[5px] shrink-0 w-3 text-right">{stepIdx + 1}</span>
                            {#if purchased}
                              <span class="flex-1 py-[5px] px-2 text-sm">{step}</span>
                            {:else}
                              <input type="text" value={step}
                                class="flex-1 py-[5px] px-2 rounded border border-transparent hover:border-base-300 focus:border-base-300 text-sm bg-transparent outline-none font-body" />
                              <button class="opacity-0 group-hover:opacity-100 hover:text-error text-sm cursor-pointer bg-transparent border-none shrink-0 mt-[5px]">&times;</button>
                            {/if}
                          </div>
                        {/each}
                        {#if !purchased}
                          <button class="text-left py-1 px-5 text-sm text-primary cursor-pointer bg-transparent border-none hover:opacity-70" onclick={() => addStep(idx)}>+ Add step</button>
                        {/if}
                      </div>
                    </div>
                    <!-- Activity actions -->
                    {#if !purchased}
                      <div class="mt-4 pt-3 border-t border-base-300 flex items-center justify-between">
                        <div>
                          {#if deleteConfirmIdx === idx}
                            <div class="flex items-center gap-2">
                              <span class="text-xs text-error">Remove this activity?</span>
                              <button class="py-1 px-2.5 rounded-md bg-error text-error-content text-xs font-medium cursor-pointer border-none hover:brightness-110 transition-all" onclick={() => removeActivity(idx)}>Remove</button>
                              <button class="py-1 px-2.5 rounded-md text-xs text-base-content/60 cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => deleteConfirmIdx = null}>Cancel</button>
                            </div>
                          {:else}
                            <button class="flex items-center gap-1 text-xs text-base-content/50 cursor-pointer bg-transparent border-none hover:text-error transition-colors" onclick={() => deleteConfirmIdx = idx}>
                              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                              Remove
                            </button>
                          {/if}
                        </div>
                        <button
                          class="flex items-center gap-1 text-xs text-base-content/50 cursor-pointer bg-transparent border-none hover:text-primary transition-colors"
                          onclick={() => duplicateActivity(idx)}
                        >
                          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                          Duplicate
                        </button>
                      </div>
                    {/if}
                  </div>
                {/if}
              </div>
            {/each}

            {#if !purchased}
              <div class="flex justify-center py-1">
                <div class="w-px h-4 bg-base-300"></div>
              </div>

              {#if showTypePicker}
                <div class="rounded-lg border border-base-300 bg-base-100 overflow-hidden">
                  <div class="flex items-center justify-between px-3 py-2 border-b border-base-300">
                    <div class="text-xs font-semibold">Choose Activity Type</div>
                    <button class="w-5 h-5 rounded flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-sm" onclick={() => showTypePicker = false}>&times;</button>
                  </div>
                  <div class="px-3 py-2 border-b border-base-300">
                    <input type="text" class="w-full py-[5px] px-2 rounded-md border border-base-300 text-xs bg-base-100 outline-none" placeholder="Search types..." bind:value={typePickerSearch} />
                  </div>
                  <div class="max-h-64 overflow-y-auto py-1">
                    {#each filteredTypes as t}
                      <button
                        class="w-full flex items-center gap-2.5 px-3 py-2 text-left cursor-pointer bg-transparent border-none hover:bg-base-200/50 transition-colors"
                        onclick={() => addTypedActivity(t.type)}
                      >
                        <div class="w-6 h-6 rounded-md bg-base-200 border {t.accentClass} flex items-center justify-center text-sm shrink-0">{t.icon}</div>
                        <div class="flex-1 min-w-0">
                          <div class="text-sm font-medium">{t.label}</div>
                          <div class="text-xs text-base-content/60 truncate">{t.description}</div>
                        </div>
                        {#if t.branches}
                          <span class="py-0 px-1 rounded bg-accent/10 text-xs text-accent font-mono shrink-0">branching</span>
                        {/if}
                      </button>
                    {/each}
                    {#if filteredTypes.length === 0}
                      <div class="px-3 py-4 text-center text-xs text-base-content/40">No types match "{typePickerSearch}"</div>
                    {/if}
                  </div>
                </div>
              {:else}
                <button class="w-full py-2.5 rounded-lg border border-dashed border-base-300 text-sm text-primary font-medium cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={addActivity}>+ Add activity</button>
              {/if}
            {/if}
          </div>
        </div>

        <!-- Connections -->
        {#if !purchased && (ew.wf.activities?.length ?? 0) > 0}
          {@const conns = ew.wf.connections ?? generateLinearConnections(ew.wf.activities ?? [], ew.wf.emit)}
          <div class="mt-5">
            <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-2">Connections</div>
            {#if conns.length > 0}
              <div class="flex flex-col gap-1">
                {#each conns as conn}
                  <div class="flex items-center gap-2 py-1.5 px-3 rounded-lg border border-base-300 bg-base-100 group text-xs">
                    <span class="font-mono text-base-content/70 truncate flex-1">{conn.from === '__trigger__' ? 'Trigger' : conn.from}</span>
                    <span class="text-base-content/30 shrink-0">&#8594;</span>
                    <span class="font-mono text-base-content/70 truncate flex-1">{conn.to === '__emit__' ? 'Emit' : conn.to}</span>
                    {#if conn.label}
                      <span class="py-0 px-1 rounded bg-accent/10 text-accent font-mono shrink-0">{conn.label}</span>
                    {/if}
                    <button
                      class="opacity-0 group-hover:opacity-100 w-4 h-4 rounded flex items-center justify-center text-base-content/40 hover:text-error cursor-pointer bg-transparent border-none shrink-0 transition-opacity"
                      title="Remove connection"
                      onclick={() => removeWorkflowConnection(conn.from, conn.to)}
                    >&times;</button>
                  </div>
                {/each}
              </div>
            {:else}
              <div class="text-xs text-base-content/50">No connections defined.</div>
            {/if}

            <!-- Add connection -->
            {#if showAddConnection}
              <div class="mt-2 rounded-lg border border-base-300 bg-base-100 p-3 flex flex-col gap-2">
                <div class="flex items-center gap-2">
                  <select class="flex-1 py-[5px] px-2 rounded-md border border-base-300 text-xs bg-base-100 outline-none font-mono" bind:value={newConnFrom}>
                    <option value="">From...</option>
                    {#each allNodeIds as nid}
                      <option value={nid}>{nid === '__trigger__' ? 'Trigger' : nid === '__emit__' ? 'Emit' : nid}</option>
                    {/each}
                  </select>
                  <span class="text-xs text-base-content/30">&#8594;</span>
                  <select class="flex-1 py-[5px] px-2 rounded-md border border-base-300 text-xs bg-base-100 outline-none font-mono" bind:value={newConnTo}>
                    <option value="">To...</option>
                    {#each allNodeIds as nid}
                      <option value={nid}>{nid === '__trigger__' ? 'Trigger' : nid === '__emit__' ? 'Emit' : nid}</option>
                    {/each}
                  </select>
                </div>
                <input type="text" class="w-full py-[5px] px-2 rounded-md border border-base-300 text-xs bg-base-100 outline-none font-mono" placeholder="Label (optional, e.g. True, False)" bind:value={newConnLabel} />
                <div class="flex items-center gap-2">
                  <button
                    class="py-1 px-2.5 rounded-md bg-primary text-primary-content text-xs font-medium cursor-pointer border-none disabled:opacity-40 disabled:cursor-not-allowed"
                    disabled={!newConnFrom || !newConnTo || newConnFrom === newConnTo}
                    onclick={() => {
                      addNewConnection(newConnFrom, newConnTo, newConnLabel || undefined);
                      showAddConnection = false;
                      newConnFrom = '';
                      newConnTo = '';
                      newConnLabel = '';
                    }}
                  >Add</button>
                  <button class="py-1 px-2.5 rounded-md text-xs text-base-content/60 cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => { showAddConnection = false; newConnFrom = ''; newConnTo = ''; newConnLabel = ''; }}>Cancel</button>
                </div>
              </div>
            {:else}
              <button
                class="mt-2 text-xs text-primary cursor-pointer bg-transparent border-none hover:opacity-70"
                onclick={() => showAddConnection = true}
              >+ Add connection</button>
            {/if}
          </div>
        {/if}
      </div>

      <!-- Footer -->
      <div class="flex items-center justify-between px-5 py-3 border-t border-base-300 shrink-0">
        {#if purchased}
          <div></div>
          <button class="py-1.5 px-3 rounded-md border border-base-300 bg-base-100 text-sm cursor-pointer hover:bg-base-200 transition-colors" onclick={() => editingWorkflow = null}>Close</button>
        {:else}
          <button class="py-1.5 px-3 rounded-md text-sm text-error cursor-pointer bg-transparent border border-base-300 hover:bg-error/10 transition-colors">Delete</button>
          <div class="flex gap-2">
            <button class="py-1.5 px-3 rounded-md border border-base-300 bg-base-100 text-sm cursor-pointer hover:bg-base-200 transition-colors" onclick={() => editingWorkflow = null}>Cancel</button>
            <button class="py-1.5 px-4 rounded-md bg-base-content text-base-100 text-sm font-medium cursor-pointer border-none" onclick={() => editingWorkflow = null}>Save</button>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

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
          onclose={() => showCanvasModal = false}
          onsave={(wfs) => { config.workflows = wfs; showCanvasModal = false; }}
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

{#if showSetupModal}
  <AgentSetupModal
    appId=""
    agentName={setupAgentName}
    agentDescription={setupAgentDesc}
    inputs={setupInputFields}
    existingAgentId={$page.params.agentId}
    onComplete={(id) => { showSetupModal = false; loadAgentData(id); }}
    onCancel={() => { showSetupModal = false; }}
  />
{/if}

<CodeInstallModal
  bind:show={showInstallModal}
  onAgentSetup={(agentId, agentName) => {
    setupAgentName = agentName;
    showSetupModal = true;
  }}
/>
