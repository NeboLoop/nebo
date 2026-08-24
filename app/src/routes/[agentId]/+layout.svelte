<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import { t } from 'svelte-i18n';
  import { setContext, onMount } from 'svelte';
  import { getWebSocketClient } from '$lib/websocket/client';
  import { AGENT_COLORS_MAP, assignAgentColors } from '$lib/tokens.js';
  import UserMenu from '$lib/components/UserMenu.svelte';
  import ChristeningModal from '$lib/components/ChristeningModal.svelte';
  import WorkflowBuilder from '$lib/components/workflow/WorkflowBuilder.svelte';
  import { launchApp } from '$lib/apps/launcher.js';
  import CollapsibleRail from '$lib/components/ui/CollapsibleRail.svelte';
  import BrandMark from '$lib/components/BrandMark.svelte';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import InboxView from '$lib/components/inbox/InboxView.svelte';
  import RunsPane from '$lib/components/flows/RunsPane.svelte';
  import RunDetail from '$lib/components/runs/RunDetail.svelte';
  import MarketplaceBrowse from '$lib/components/marketplace/MarketplaceBrowse.svelte';
  import CategoryRail, { hasCategoryRail } from '$lib/components/marketplace/CategoryRail.svelte';
  import ProductDetail from '$lib/components/marketplace/ProductDetail.svelte';
  import CoworkerThreadView from '$lib/components/chat/CoworkerThreadView.svelte';
  import WorkroomView from '$lib/components/workrooms/WorkroomView.svelte';
  import AgentSettingsModal from '$lib/components/settings/agent/AgentSettingsModal.svelte';
  import ConfirmModal from '$lib/components/settings/ConfirmModal.svelte';
  import NewEmployeeModal from '$lib/components/NewEmployeeModal.svelte';
  import { unreadCount } from '$lib/stores/notifications';
  import { slide } from 'svelte/transition';
  import { logger } from '$lib/monitoring';
  import MessageSquareLock from 'lucide-svelte/icons/message-square-lock';

  // Sidebar drill choreography: siblings collapse, the clicked row rides to
  // the top, conversations expand under it. Zero-duration under
  // prefers-reduced-motion.
  const motionMs = (ms: number) =>
    typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : ms;

  // The list is URL state — SvelteKit-standard, not a side store. `?list=1`
  // is the roster, `?list=<agentId>` is that employee's matter list, absent
  // means the conversation. Every transition is a goto, so the browser back
  // button walks the screens the user actually saw, and each screen has a
  // real URL. (Store-driven drawers were the bug: tapping back showed the
  // list without changing the URL, then browser-back navigated somewhere
  // stale underneath it.)
  const listParam = $derived($page.url.searchParams.get('list'));

  function listUrl(value: string | null): string {
    const url = new URL($page.url);
    if (value) url.searchParams.set('list', value);
    else url.searchParams.delete('list');
    return url.pathname + url.search;
  }
  const showList = (value: string, replace = false) =>
    goto(listUrl(value), { replaceState: replace, noScroll: true, keepFocus: true });
  const closeList = () => goto(listUrl(null), { noScroll: true, keepFocus: true });

  // Drawer visibility below md is exactly "is a list screen in the URL".
  const listOpen = $derived($page.url.searchParams.has('list'));

  /**
   * What a row click does is decided by memory isolation, because that is what
   * separate conversations MEAN.
   *
   * Isolation on — each conversation keeps its own sealed memory, so the list
   * is a list of matters and you pick one. Isolation off — one memory across
   * every chat, so there is one continuous conversation and we open it.
   */
  async function openAgentRow(id: string) {
    let a = allAgents.find((x) => x.id === id);
    // Isolation decides whether this tap lands on the matter list, and the
    // flag is lazy (the list endpoint has no frontmatter). Unknown → resolve
    // it FIRST, then navigate once — a tap must not guess and correct itself.
    if (a && a.isolated === undefined) {
      try {
        const api = await import('$lib/api/nebo');
        const resp = (await api.getAgent(id)) as { agent?: { frontmatter?: string } };
        const iso = JSON.parse(resp.agent?.frontmatter || '{}')?.memory?.context_isolated === true;
        const idx = allAgents.findIndex((x) => x.id === id);
        if (idx !== -1) {
          const next = [...allAgents];
          next[idx] = { ...next[idx], isolated: iso };
          allAgents = next;
          a = next[idx];
        }
      } catch { /* unknown stays unknown; fall through to plain open */ }
    }
    selectAgent(id, a?.isolated ? id : null);
  }

  // Shelf surfaces open over the workspace rather than navigating away from
  // it. State is in the URL so each one deep-links and the back button works.
  const inboxOpen = $derived($page.url.searchParams.has('inbox'));
  const inboxSelected = $derived($page.url.searchParams.get('m'));

  function setParams(mut: (p: URLSearchParams) => void, replace = false) {
    const url = new URL($page.url);
    mut(url.searchParams);
    goto(url.pathname + url.search, { replaceState: replace, noScroll: true, keepFocus: true });
  }
  // The view-only employee↔employee transcript (WS5 audit surface). URL
  // param carries the coworker thread's session key:
  // agent:{target}:coworker:{sender}[:{matter}].
  const cwKey = $derived($page.url.searchParams.get('cw'));
  const closeCoworkerThread = () => setParams((p) => { p.delete('cw'); p.delete('cwf'); });
  const cwNames = $derived.by(() => {
    if (!cwKey) return null;
    const parts = cwKey.split(':');
    const nameOf = (id: string) =>
      !id || id === 'main' || id === 'assistant'
        ? 'Nebo'
        : (allAgents.find((a) => a.id === id)?.name ?? null);
    // Sender: the chip passes the display name (?cwf=) because an isolated
    // sender's key carries its MATTER id, not its agent id. Fall back to a
    // roster match on the context segment, then to this page's agent (chips
    // live in the sender's own chat).
    const sender =
      $page.url.searchParams.get('cwf') ??
      nameOf(parts[3] ?? '') ??
      agent?.name ??
      'Nebo';
    return { target: nameOf(parts[1] ?? '') ?? (parts[1] || 'Nebo'), sender };
  });

  const settingsSection = $derived($page.url.searchParams.get('settings'));
  const runsOpen = $derived($page.url.searchParams.has('runs'));
  const openRunId = $derived($page.url.searchParams.get('run'));
  // Keep `runs` in place: the run detail stacks OVER the run list, so its
  // back/close returns to the list instead of dumping the owner to the chat.
  const openRun = (id: string) => setParams((p) => p.set('run', id));
  const closeRun = () => setParams((p) => p.delete('run'));
  const marketOpen = $derived($page.url.searchParams.has('market'));
  const openMarket = () => setParams((p) => p.set('market', '1'));
  const closeMarket = () => setParams((p) => p.delete('market'));

  // The marketplace modal is a full storefront, not a teaser: it browses every
  // kind and opens product detail IN PLACE. Card components keep their plain
  // /marketplace/... hrefs (the route tree still serves deep links); one
  // capture-phase handler on the modal body reroutes those clicks to modal
  // state instead of navigating the workspace away.
  const MARKET_KINDS = [
    { id: 'employees', labelKey: 'marketplace.nav.employees' },
    { id: 'tools', labelKey: 'marketplace.nav.tools' },
    { id: 'collections', labelKey: 'marketplace.nav.collections' },
  ];
  const MARKET_PLURAL_TO_TYPE: Record<string, 'agent' | 'app' | 'skill' | 'plugin' | 'connector' | 'collection'> = {
    agents: 'agent', apps: 'app', skills: 'skill', plugins: 'plugin', connectors: 'connector', collections: 'collection',
  };
  // Where in the storefront the modal is standing. Same shape the /marketplace
  // route reads out of its URL, so both mounts feed MarketplaceBrowse the same
  // way — the modal just keeps it in state instead of the address bar.
  type MarketLoc = { kind: string; price: string; category: string; publisher: string; filter: string };
  const MARKET_HOME: MarketLoc = { kind: 'employees', price: 'all', category: '', publisher: '', filter: '' };
  let market = $state<MarketLoc>({ ...MARKET_HOME });
  let marketDetail = $state<{ id: string; type: 'agent' | 'app' | 'skill' | 'plugin' | 'connector' | 'collection' } | null>(null);
  // Reopening the modal starts at browse, not wherever it was left.
  $effect(() => { if (!marketOpen) { marketDetail = null; market = { ...MARKET_HOME }; } });

  function marketLocFrom(url: URL): MarketLoc {
    const p = url.searchParams;
    const category = p.get('category') ?? '';
    const publisher = p.get('publisher') ?? '';
    return {
      kind: p.get('kind') || (category || publisher ? 'all' : 'employees'),
      price: p.get('price') || 'all',
      category,
      publisher,
      filter: p.get('filter') ?? '',
    };
  }

  function interceptMarketClick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest('a[href]');
    if (!a) return;
    const href = a.getAttribute('href') ?? '';
    if (!href.startsWith('/marketplace')) return;
    const url = new URL(href, location.origin);
    const detail = url.pathname.match(/^\/marketplace\/([a-z]+)\/([^/]+)\/?$/);
    const isStorefront = url.pathname.replace(/\/$/, '') === '/marketplace';
    // Detail pages and the storefront (with any of its filters) are what the
    // modal renders. Other screens — /marketplace/installed, /marketplace/shared
    // — still live on the route tree and navigate for real.
    if (!isStorefront && !(detail && MARKET_PLURAL_TO_TYPE[detail[1]])) return;
    e.preventDefault();
    e.stopPropagation();
    if (detail) {
      marketDetail = { id: decodeURIComponent(detail[2]), type: MARKET_PLURAL_TO_TYPE[detail[1]] };
      return;
    }
    marketDetail = null;
    market = marketLocFrom(url);
  }

  function selectMarketKind(kind: string) {
    marketDetail = null;
    market = { ...MARKET_HOME, kind };
  }
  // ── Workrooms: mission rooms where the owner and several employees share
  // one conversation (a room IS a loop channel; the hub owns the history).
  // `?room=<channelId>` opens a room, `?room=new` is the create form.
  let workrooms = $state<import('$lib/api/neboComponents').Workroom[]>([]);
  // Live recency: a room row rises within its section when its channel talks.
  let roomActivity = $state<Record<string, number>>({});
  async function loadWorkrooms() {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listWorkrooms();
      workrooms = resp?.workrooms ?? [];
    } catch {
      /* section simply stays absent */
    }
  }
  const roomParam = $derived($page.url.searchParams.get('room'));
  const openRoomObj = $derived(workrooms.find((w) => w.channelId === roomParam) ?? null);
  const sortedWorkrooms = $derived(
    [...workrooms].sort(
      (a, b) =>
        (roomActivity[b.channelId] ?? b.createdAt * 1000) -
        (roomActivity[a.channelId] ?? a.createdAt * 1000)
    )
  );
  const openRoom = (id: string) => setParams((p) => p.set('room', id));
  const closeRoom = () => setParams((p) => p.delete('room'));
  // First two members for the row's stacked-avatars glyph, in their roster
  // colors — the room row previews who's inside.
  const roomFaces = (room: { memberAgentIds: string[] }) =>
    room.memberAgentIds
      .map((id) => allAgents.find((a) => a.id === id))
      .filter((a): a is (typeof allAgents)[number] => !!a)
      .slice(0, 2)
      .map((a) => {
        const ac = AGENT_COLORS_MAP[a.color] ?? AGENT_COLORS_MAP['teal'];
        return { initial: a.initial, cls: `${ac.bgClass} ${ac.inkClass}` };
      });

  // Room housekeeping: right-click → Remove forgets the registration (the
  // conversation history stays on the hub); confirm first — it's an audit
  // surface leaving the sidebar.
  let newEmployeeOpen = $state(false);
  let roomCtxMenu = $state<{ x: number; y: number; channelId: string } | null>(null);
  let removeRoom = $state<import('$lib/api/neboComponents').Workroom | null>(null);
  let removeRoomBusy = $state(false);
  function handleRoomContext(e: MouseEvent, channelId: string) {
    e.preventDefault();
    roomCtxMenu = { x: e.clientX, y: e.clientY, channelId };
  }
  async function confirmRemoveRoom() {
    const room = removeRoom;
    if (!room || removeRoomBusy) return;
    removeRoomBusy = true;
    try {
      const api = await import('$lib/api/nebo');
      await api.deleteWorkroom(room.channelId);
      workrooms = workrooms.filter((w) => w.channelId !== room.channelId);
      if (roomParam === room.channelId) closeRoom();
      removeRoom = null;
    } catch { /* row stays; the owner can retry */ }
    removeRoomBusy = false;
  }

  // The value doubles as the initial status filter ("failed") so the stat
  // tiles can deep-link straight to what they count; '1' = unfiltered.
  const openRuns = (filter?: string) =>
    setParams((p) => p.set('runs', filter === 'failed' || filter === 'running' ? filter : '1'));
  const closeRuns = () => setParams((p) => p.delete('runs'));
  const openInbox = () => setParams((p) => p.set('inbox', '1'));
  const openSettings = () => setParams((p) => p.set('settings', 'general'));
  const closeSettings = () => setParams((p) => p.delete('settings'));
  // Section-to-section is not a history step; Escape should close the modal,
  // not walk back through eleven sections.
  const selectSection = (id: string) => setParams((p) => p.set('settings', id), true);
  const closeInbox = () => setParams((p) => { p.delete('inbox'); p.delete('m'); });
  const selectInboxItem = (id: string | null) =>
    setParams((p) => (id ? p.set('m', id) : p.delete('m')), true);

  /**
   * Authoring a flow is a conversation, so "ask" drops a starter prompt into
   * the employee's composer rather than opening an editor. Prefill, don't send:
   * the owner finishes the sentence.
   */
  function askEmployee(prompt: string) {
    if (!agentId) return;
    const url = new URL($page.url);
    url.searchParams.set('ask', prompt);
    goto(url.pathname + url.search, { noScroll: true });
  }

  /** Mail-client recency: time today, weekday this week, date beyond. */
  function dayLabel(epochSecs: number): string {
    if (!epochSecs) return '';
    const d = new Date(epochSecs * 1000);
    const days = (Date.now() - d.getTime()) / 86_400_000;
    if (days < 1) return d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
    if (days < 7) return d.toLocaleDateString(undefined, { weekday: 'long' });
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  }
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
  let primaryChristened = $state(true);
  let threadsLoading = $state<Record<string, boolean>>({});


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
      // First-contact gate (owner decision: not skippable): until the primary
      // is named, the workspace waits behind the christening ceremony. Cloud
      // installs hit this on first workspace mount — the wizard never runs there.
      primaryChristened = (agentsResp as any)?.primaryChristened !== false;
      if (agentsResp?.agents?.length) {
        const agents = agentsResp.agents;
        const colors = assignAgentColors(agents);
        allAgents = agents.map(a => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: activeIds.has(a.id) ? 'online' : 'paused',
          color: colors[a.id],
          handle: a.handle,
          editable: !a.nappPath,
          isApp: a.isApp ?? false,
          loopExposed: a.loopExposed ?? false,
          loopAgentId: a.loopAgentId,
          voice: a.voice || '',
          // The list endpoint reports isolation directly (the roster lock
          // needs it for every row).
          isolated: a.isolated,
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

  // WS subscriptions on the single ws.on pathway (no window-event bridge). These
  // are set up in onMount, so we collect unsubscribes and tear them down in the
  // onMount return. (The shared onWsEvent helper is for top-level subscriptions.)
  const wsUnsubs: (() => void)[] = [];

  function onWsEvent(event: string, handler: (data: any) => void) {
    wsUnsubs.push(getWebSocketClient().on(event.replace(/^nebo:/, ''), handler));
  }

  onMount(() => {
    // Initial roster load
    loadAgentRoster();
    loadWorkrooms();

    // Phones start on the team list. Landing cold on a specific conversation
    // (a shared link, a notification) keeps that conversation — the list is
    // one back-chevron away — but the bare new-chat landing shows the list,
    // because that is the decision the user is actually making.
    if (
      typeof window !== 'undefined' &&
      !window.matchMedia('(min-width: 768px)').matches &&
      !$page.params.threadId &&
      !$page.url.searchParams.has('list')
    ) {
      showList('1', true);
    }

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

    // Rooms are opened by employees; the sidebar learns about a new one live.
    onWsEvent('nebo:workroom_created', (data) => {
      const room = data?.workroom;
      if (!room?.channelId) return;
      workrooms = [room, ...workrooms.filter((w) => w.channelId !== room.channelId)];
      roomActivity[room.channelId] = Date.now();
    });
    // Workroom traffic → bump that room's recency in the sidebar section.
    // The open room view holds its own subscription for the transcript.
    onWsEvent('nebo:workroom_message', (data) => {
      if (data?.channelId) roomActivity[data.channelId] = Date.now();
    });

    // Run/workflow updates → refresh runs + stats
    onWsEvent('nebo:run_update', () => refreshRuns());
    onWsEvent('nebo:workflow_update', () => refreshRuns());
    onWsEvent('nebo:workflow_run_started', () => refreshRuns());
    onWsEvent('nebo:workflow_run_completed', () => refreshRuns());
    onWsEvent('nebo:workflow_run_failed', () => refreshRuns());

    return () => {
      for (const off of wsUnsubs) off();
      wsUnsubs.length = 0;
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
      logger.debug(`[nebo] import api: ${(performance.now() - t0).toFixed(0)}ms`);
      // Fire all requests in parallel but resolve threads first to unblock the UI
      const chatsPromise = api.listAgentChats(id).then(r => { logger.debug(`[nebo] chats: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] listAgentChats failed for', id, e); return null; });
      const runsPromise = api.listAgentRuns(id).then(r => { logger.debug(`[nebo] runs: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] listAgentRuns failed for', id, e); return null; });
      const statsPromise = api.agentStats(id).then(r => { logger.debug(`[nebo] stats: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] agentStats failed for', id, e); return null; });
      const agentPromise = api.getAgent(id).then(r => { logger.debug(`[nebo] agent: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] getAgent failed for', id, e); return null; });
      const workflowsPromise = api.listAgentWorkflows(id).then(r => { logger.debug(`[nebo] workflows: ${(performance.now() - t0).toFixed(0)}ms`); return r; }).catch((e: unknown) => { console.warn('[nebo] listAgentWorkflows failed for', id, e); return null; });

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
        // Isolation lives in frontmatter, which only getAgent returns. Learn
        // it here and patch the roster row; if we learn mid-view that the
        // employee is isolated, show their matter list — that is what a row
        // click would have done had we known.
        try {
          const fm = JSON.parse((ar.agent as Agent)?.frontmatter || '{}');
          const iso = fm?.memory?.context_isolated === true;
          const idx = allAgents.findIndex((x) => x.id === id);
          if (idx !== -1 && allAgents[idx].isolated !== iso) {
            const next = [...allAgents];
            next[idx] = { ...next[idx], isolated: iso };
            allAgents = next;
            if (iso && $page.params.agentId === id && $page.url.searchParams.get('list') === '1') showList(id, true);
          }
        } catch { /* malformed frontmatter — leave unknown */ }
        const persona = typeof ar.persona === 'string' ? (ar.persona as string) : '';
        const agentMd = (ar.agent as Agent)?.agentMd || '';
        const soul = (ar.agent as Agent)?.soul || '';
        const rules = (ar.agent as Agent)?.rules || '';
        const model = typeof ar.model === 'string' ? ar.model : (ar.model as Record<string, unknown>)?.id as string ?? 'claude-sonnet-4-6';
        const inputs = Array.isArray(ar.inputFields) ? ar.inputFields as Record<string, unknown>[] : [];
        // Workflows from separate endpoint — merged below
        apiConfig[id] = { persona, agentMd, soul, rules, model, inputs, workflows: apiConfig[id]?.workflows ?? {} };
        // Post-install configuration lives in Settings → Configure (the one canonical
        // surface). We do NOT auto-open a setup modal here: doing so created an endless
        // loop (modal close → reload → needsSetup still true → reopen). First-run setup
        // happens in the install flow; unfinished required inputs are edited in Settings.
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
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgentRuns(id, 20, current);
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

  // One list: employees, then apps.
  const listedAgents = $derived([...sortedAgents, ...sortedAppAgents]);

  const agentId = $derived($page.params.agentId ?? '');
  const agent = $derived(allAgents.find(a => a.id === agentId));
  // A stale deep link — the employee was reinstalled (new id) or deleted from
  // another surface — must not strand the owner on a half-broken page fanning
  // 404s. Once the roster is authoritative and the id isn't in it, go home.
  $effect(() => {
    if (!agentsLoading && allAgents.length > 0 && agentId && !allAgents.some((a) => a.id === agentId)) {
      goto('/');
    }
  });
  // For an ISOLATED employee, no list param keeps their matter list in the
  // column: clicking a matter navigates to the thread (dropping the param,
  // which is what closes the phone drawer) and the column must NOT snap back
  // to the roster. `list=1` stays the explicit way out to the roster.
  const drilledAgentId = $derived(
    listParam && listParam !== '1' ? listParam : !listParam && agent?.isolated ? agentId : null
  );
  const drilledAgent = $derived(drilledAgentId ? allAgents.find(a => a.id === drilledAgentId) ?? null : null);
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


  async function selectAgent(id: string, list: string | null = null) {
    const a = allAgents.find(ag => ag.id === id);
    // Apps are employees too: they open a conversation like everyone else,
    // with Open App available in the chat header. The old /overview landing
    // was a dead end — no chat, and on a phone no way back.
    // Open the employee's most recent conversation, not a blank new chat.
    // /threads is the NEW-chat page: with the chat list drilled away, sending
    // a row click there stranded every employee who had exactly one
    // conversation — no drill chevron, and their chat unreachable.
    // The `+` button is what creates a new one.
    let latest = apiThreads[id]?.[0];
    if (!latest) {
      // Never visited, so we have no preview for them yet. One request, which
      // also fills in their row's preview.
      const api = await import('$lib/api/nebo');
      const r = await api.listAgentChats(id).catch(() => null);
      if (r?.chats?.length) {
        apiThreads[id] = r.chats as EnrichedChat[];
        latest = apiThreads[id][0];
      }
    }
    const suffix = list ? `?list=${encodeURIComponent(list)}` : '';
    goto((latest ? `/${id}/threads/${latest.id}` : `/${id}/threads`) + suffix);
  }

  // Returns an i18n key — translate with $t at the call site.
  function statusLabel(s: string) {
    if (s === 'online') return 'common.online';
    if (s === 'running') return 'agent.running';
    if (s === 'paused') return 'common.paused';
    return 'agent.idle';
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
      deleteTarget = { id, name: a?.name || '' };
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
    openRuns,
    openSettings,
    openList: () => showList(agent?.isolated ? agentId : '1'),
    askEmployee,
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

<svelte:head><title>{agent?.name ?? $t('common.agent')} - Nebo</title></svelte:head>

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
        {$t('agent.openApp')}
      </button>
      <div class="h-px bg-base-300 my-1"></div>
    {:else}
      {#if ctxMenu.agentId !== 'assistant'}
        <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('toggle-status')}>
          {#if ctxSt === 'paused'}
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" class="text-success"><polygon points="6,4 20,12 6,20"/></svg>
            {$t('agent.activate')}
          {:else}
            <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" class="text-base-content/50"><rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/></svg>
            {$t('sidebar.pause')}
          {/if}
        </button>
        <div class="h-px bg-base-300 my-1"></div>
      {/if}
      <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('new-thread')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/50"><line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/></svg>
        {$t('agent.newChat')}
      </button>
    {/if}
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('copy-id')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" class="text-base-content/50"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
      {$t('agent.copyAgentId')}
    </button>
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => ctxAction('settings')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/50"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
      {$t('nav.settings')}
    </button>
    <div class="h-px bg-base-300 my-1"></div>
    {#if ctxAgent?.editable}
      <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-error/10 text-error transition-colors" onclick={() => ctxAction('delete')}>
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
        {$t('common.delete')}
      </button>
    {/if}
  </div>
{/if}

<!-- Workroom context menu — one action: housekeeping. -->
{#if roomCtxMenu}
  <div class="fixed inset-0 z-50" onclick={() => (roomCtxMenu = null)} oncontextmenu={(e) => { e.preventDefault(); roomCtxMenu = null; }} role="presentation"></div>
  <div
    class="fixed z-50 w-[180px] py-1 rounded-lg border border-base-300 bg-base-100 shadow-xl"
    style="left: {roomCtxMenu.x}px; top: {roomCtxMenu.y}px;"
  >
    <button
      class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-error/10 text-error transition-colors"
      onclick={() => { removeRoom = workrooms.find((w) => w.channelId === roomCtxMenu?.channelId) ?? null; roomCtxMenu = null; }}
    >
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
      {$t('workrooms.remove')}
    </button>
  </div>
{/if}

{#if removeRoom}
  <ConfirmModal
    title={$t('workrooms.removeTitle', { values: { name: removeRoom.name } })}
    message={$t('workrooms.removeBody')}
    confirmLabel={$t('workrooms.remove')}
    busy={removeRoomBusy}
    onConfirm={confirmRemoveRoom}
    onCancel={() => (removeRoom = null)}
  />
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
          <h3 class="text-sm font-bold">{$t('agent.deleteTitle', { values: { name: deleteTarget.name || $t('agent.thisAgent') } })}</h3>
          <p class="text-xs text-base-content/50">{$t('agent.cannotBeUndone')}</p>
        </div>
      </div>
      <div class="px-5 py-4">
        <p class="text-sm text-base-content/70">{$t('agent.deleteWarning')}</p>
      </div>
      <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
        <button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent" onclick={() => { deleteTarget = null; }} disabled={deleting}>{$t('common.cancel')}</button>
        <button class="px-4 py-2 rounded-lg bg-error text-error-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none" onclick={confirmDeleteAgent} disabled={deleting}>
          {#if deleting}
            <span class="loading loading-spinner loading-xs"></span>
          {:else}
            {$t('agentSettings.deleteAgent')}
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- Workflow editor modal -->
<!-- Workflow canvas builder — full-screen overlay -->
{#if showCanvasModal}
  <div class="fixed inset-0 z-[75] flex flex-col" data-modal-open>
    <div class="absolute inset-0 bg-black/40" role="presentation"></div>
    <div class="relative flex flex-col flex-1 m-4 rounded-2xl bg-base-100 border border-base-300 shadow-2xl z-10 overflow-hidden">
      <div class="flex items-center justify-between px-5 py-3 border-b border-base-content/10 shrink-0">
        <div class="flex items-center gap-3">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="8" y="14" width="7" height="7" rx="1"/><line x1="6.5" y1="10" x2="11.5" y2="14"/><line x1="17.5" y1="10" x2="11.5" y2="14"/></svg>
          <div>
            <div class="text-sm font-semibold">{$t('agent.workflowBuilder', { values: { name: agent?.name ?? '' } })}</div>
            <div class="text-xs text-base-content/50">{$t('agent.workflowsActivitiesCount', { values: { workflows: workflowEntries.length, activities: workflowEntries.reduce((sum, [, wf]) => sum + (wf.activities?.length ?? 0), 0) } })}</div>
          </div>
        </div>
        <button class="w-8 h-8 rounded-lg flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-lg" onclick={() => showCanvasModal = false}>&times;</button>
      </div>
      <div class="flex-1 min-h-0">
        <WorkflowBuilder
          workflows={config.workflows}
          agentId={agentId}
          agentName={agent?.name ?? $t('common.agent')}
          focusWorkflow={canvasFocusWorkflow}
          onclose={() => { showCanvasModal = false; canvasFocusWorkflow = null; }}
          onopensettings={(section) => setParams((p) => p.set('settings', section))}
          onsave={(wfs) => { showCanvasModal = false; canvasFocusWorkflow = null; persistWorkflows(wfs).catch((e) => console.error('[nebo] failed to save workflows', e)); }}
        />
      </div>
    </div>
  </div>
{/if}

<!-- The workspace list: Inbox, then the roster, each employee expanding to
     their chats. This is the app's only navigation. -->
<CollapsibleRail
  section="workspace"
  title="Nebo"
  mobileOpen={listOpen}
  onmobileclose={closeList}
  tour="agents"
>
  {#snippet leading()}
    <a href="/" class="shrink-0 flex items-center text-base-content" title="Nebo">
      <BrandMark class="w-5 h-5" />
    </a>
  {/snippet}

  {#snippet headerActions()}
    <!-- Search hidden for now (owner call, 2026-08-23) — ⌘K still opens the
         command palette; only the header button is gone. -->
    <button
      class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-100 cursor-pointer bg-transparent border-none shrink-0"
      onclick={() => (newEmployeeOpen = true)}
      title={$t('newEmployee.title')}
    >
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><line x1="8" y1="3" x2="8" y2="13"/><line x1="3" y1="8" x2="13" y2="8"/></svg>
    </button>
  {/snippet}

  {#snippet expanded()}
    {#if agentsLoading && sortedAgents.length === 0}
      <div class="py-6 flex items-center justify-center">
        <span class="loading loading-spinner loading-sm"></span>
      </div>
    {:else}
      <!-- ONE keyed list for both states. Drilling filters it to the clicked
           employee, so that row's DOM node survives and rides to the top as
           the siblings collapse (transition:slide) — the container-transform
           feel: the row you clicked BECOMES the header, nothing teleports.
           Clicking the pinned row slides everyone back. -->
      {#each (drilledAgent ? [drilledAgent] : listedAgents) as a (a.id)}
        {@const st = agentStatus(a.id)}
        {@const ac = AGENT_COLORS_MAP[a.color] ?? AGENT_COLORS_MAP['teal']}
        {@const chats = apiThreads[a.id] ?? []}
        {@const latest = chats[0]}
        {@const isPinned = drilledAgent?.id === a.id}
        <div
          transition:slide={{ duration: motionMs(200) }}
          class="group/agent flex items-center gap-2.5 py-2 px-2.5 mx-1.5 cursor-pointer transition-colors text-left {!isPinned && agentId === a.id
            ? 'rounded-box border border-primary/30 bg-primary/10 shadow-sm'
            : 'rounded-box border border-transparent hover:bg-base-100/70'}"
        >
          <button
            class="flex items-center gap-2.5 flex-1 min-w-0 bg-transparent border-none cursor-pointer p-0 text-left"
            onclick={() => (isPinned ? showList('1') : openAgentRow(a.id))}
            oncontextmenu={(e) => handleAgentContext(e, a.id)}
            data-context-menu
            title={isPinned ? $t('common.back') : undefined}
          >
            <div class="relative shrink-0">
              <div class="w-8 h-8 rounded-field flex items-center justify-center font-mono text-sm font-semibold {ac.bgClass} {ac.inkClass} {st === 'paused' ? 'opacity-50' : ''}">
                {#if a.isApp}
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/></svg>
                {:else}{a.initial}{/if}
              </div>
              <div class="absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full border-2 border-base-200 {st === 'running' ? 'bg-warning animate-pulse' : st === 'paused' ? 'bg-base-content/30' : 'bg-success'}"></div>
            </div>
            <div class="flex-1 min-w-0">
              <div class="flex items-baseline gap-2">
                <span class="text-sm font-medium truncate min-w-0">{a.name}</span>
                {#if a.isolated}
                  <!-- Sealed-conversations employee: the same glyph the chat
                       header shows (MessageSquareLock, not the Settings Lock). -->
                  <span class="self-center text-warning/70 shrink-0 tooltip tooltip-right" data-tip={$t('agentIsolation.isolated')}>
                    <MessageSquareLock class="w-2.5 h-2.5" />
                  </span>
                {/if}
                {#if a.isApp}
                  <span class="text-[9px] uppercase tracking-wider px-1 py-px rounded bg-info/15 text-info font-semibold shrink-0">{$t('agent.appBadge')}</span>
                {/if}
                <span class="flex-1"></span>
                {#if isPinned}
                  <!-- Open-disclosure chevron: this row is the way back. -->
                  <svg class="self-center text-base-content/50" width="12" height="12" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 8 11 13 6"/></svg>
                {/if}
                <!-- No per-row time here: employees without chats have none,
                     and a column where only some rows carry a time reads as
                     noise. Times live on the conversations themselves. -->
              </div>
              <div class="text-xs text-base-content/60 truncate">{latest?.preview || a.role}</div>
            </div>
          </button>
        </div>
      {/each}
      {#if drilledAgent}
        <!-- The pinned employee's conversations, expanding beneath their row.
             In waits for the sibling rows to finish collapsing. -->
        <div
          in:slide={{ duration: motionMs(220), delay: motionMs(140) }}
          out:slide={{ duration: motionMs(150) }}
        >
          <div class="h-px bg-base-content/8 mx-3 mb-1"></div>
          <!-- Isolation means each conversation is its own sealed matter, so
               starting a new one has to be reachable from the list of them. -->
          <a
            href={`/${drilledAgent.id}/threads`}
            class="flex items-center gap-2 py-2 px-2.5 mx-1.5 mb-1 rounded-box border border-dashed border-base-300 text-primary hover:bg-base-100/70 transition-colors"
          >
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><line x1="8" y1="3" x2="8" y2="13"/><line x1="3" y1="8" x2="13" y2="8"/></svg>
            <span class="text-sm font-medium">{$t('agent.newChat')}</span>
          </a>
          {#each apiThreads[drilledAgent.id] ?? [] as c (c.id)}
            <a
              href={`/${drilledAgent.id}/threads/${c.id}`}
              class="block py-2 px-2.5 mx-1.5 rounded-box transition-colors {$page.params.threadId === c.id
                ? 'bg-primary/10 border border-primary/30 shadow-sm'
                : 'border border-transparent hover:bg-base-100/70'}"
            >
              <div class="flex items-baseline gap-2">
                <span class="text-sm truncate flex-1 min-w-0">{c.title || c.name}</span>
                <span class="text-xs text-base-content/45 shrink-0">{dayLabel(c.updatedAtEpoch)}</span>
              </div>
              <div class="text-xs text-base-content/55 truncate">{c.preview}</div>
            </a>
          {/each}
        </div>
      {/if}
      {#if !drilledAgent}
        <!-- WORKROOMS — mission rooms, under the employees (list = conversations;
             the shelf is for utilities). Rooms are opened by employees, never by
             a form: whichever employee owns a task creates the room and brings
             coworkers in. So the empty state is NOTHING — the section exists
             the day work starts happening in one. -->
        <div class="flex items-center gap-2 mt-4 mb-1 mx-4">
          <span class="text-[10px] font-semibold uppercase tracking-wider text-base-content/45">{$t('workrooms.section')}</span>
        </div>
        {#if sortedWorkrooms.length > 0}
          {#each sortedWorkrooms as room (room.channelId)}
            {@const faces = roomFaces(room)}
            <!-- Margin lives on the wrapper, width on the button — w-full plus
                 mx on the same box overflows the column and summons a
                 scrollbar on hover (the row "jump"). -->
            <div class="mx-1.5">
            <button
              class="group/room w-full flex items-center gap-2.5 py-2 px-2.5 cursor-pointer transition-colors text-left bg-transparent {roomParam === room.channelId
                ? 'rounded-box border border-primary/30 bg-primary/10 shadow-sm'
                : 'rounded-box border border-transparent hover:bg-base-100/70'}"
              onclick={() => openRoom(room.channelId)}
              oncontextmenu={(e) => handleRoomContext(e, room.channelId)}
            >
              <!-- Stacked-avatars glyph in the avatar slot: this row is a room,
                   not a person. -->
              <div class="relative w-8 h-8 shrink-0">
                {#if faces.length >= 2}
                  {@const extra = room.memberAgentIds.length - 2}
                  <div class="absolute top-0 left-0 w-6 h-6 rounded-field flex items-center justify-center font-mono text-[10px] font-semibold {faces[0].cls}">{faces[0].initial}</div>
                  <div class="absolute bottom-0 right-0 w-6 h-6 rounded-field border border-base-100 flex items-center justify-center font-mono text-[10px] font-semibold {faces[1].cls}">{faces[1].initial}</div>
                  {#if extra > 0}
                    <!-- The row says "several"; the full roster is the room's member rail. -->
                    <div class="absolute -top-1 -right-1 min-w-4 h-4 px-0.5 rounded-full bg-neutral text-neutral-content border border-base-100 flex items-center justify-center font-mono text-[9px] font-semibold">+{extra}</div>
                  {/if}
                {:else}
                  <div class="w-8 h-8 rounded-field bg-base-200 flex items-center justify-center text-base-content/70">
                    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>
                  </div>
                {/if}
              </div>
              <div class="flex-1 min-w-0">
                <div class="flex items-baseline gap-2">
                  <span class="text-sm font-medium truncate min-w-0">{room.name}</span>
                  <span class="flex-1"></span>
                  <span class="text-xs text-base-content/45 shrink-0">{dayLabel(roomActivity[room.channelId] ? roomActivity[room.channelId] / 1000 : room.createdAt)}</span>
                </div>
                <div class="text-xs text-base-content/60 truncate">{room.mission || $t('workrooms.membersCount', { values: { count: room.memberAgentIds.length } })}</div>
              </div>
            </button>
            </div>
          {/each}
        {:else}
          <!-- The section teaches what rooms ARE before the first one exists —
               the owner shouldn't have to ask "how do I see the rooms". -->
          <p class="text-xs text-base-content/45 mx-4 mb-1 leading-relaxed">{$t('workrooms.emptyRoster')}</p>
        {/if}
      {/if}
    {/if}
  {/snippet}

  {#snippet collapsed()}
    <div class="flex flex-col items-center gap-1 py-2">
      {#each sortedAgents.concat(sortedAppAgents) as a (a.id)}
        {@const st = agentStatus(a.id)}
        {@const ac = AGENT_COLORS_MAP[a.color] ?? AGENT_COLORS_MAP['teal']}
        <div class="relative">
          <button
            class="w-8 h-8 rounded-field flex items-center justify-center font-mono text-sm font-semibold shrink-0 cursor-pointer transition-colors border-none {ac.bgClass} {ac.inkClass} {agentId === a.id ? 'ring-2 ring-base-content/40' : ''} {st === 'paused' ? 'opacity-50' : ''}"
            onclick={() => selectAgent(a.id)}
            oncontextmenu={(e) => handleAgentContext(e, a.id)}
            data-context-menu
            title="{a.name} — {$t(statusLabel(st))}"
          >{a.initial}</button>
          <div class="absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full border-2 border-base-200 {st === 'running' ? 'bg-warning animate-pulse' : st === 'paused' ? 'bg-base-content/30' : 'bg-success'}"></div>
        </div>
      {/each}
    </div>
  {/snippet}

  {#snippet footer(isRail)}
    <div class="border-t border-base-300 shrink-0">
      <button
        type="button"
        onclick={openInbox}
        class="relative w-full flex items-center gap-2.5 py-2 {isRail ? 'justify-center px-0' : 'px-3.5'} hover:bg-base-100/70 transition-colors bg-transparent border-none cursor-pointer text-left {inboxOpen ? 'text-base-content' : ''}"
        title={$t('nav.inbox')}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class="shrink-0"><path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/></svg>
        {#if !isRail}<span class="text-sm flex-1">{$t('nav.inbox')}</span>{/if}
        {#if $unreadCount > 0}
          {#if isRail}
            <span class="absolute top-1.5 right-3 w-2 h-2 rounded-full bg-error"></span>
          {:else}
            <span class="badge badge-error badge-xs text-error-content font-semibold shrink-0">{$unreadCount > 9 ? '9+' : $unreadCount}</span>
          {/if}
        {/if}
      </button>
      <button
        type="button"
        onclick={openMarket}
        class="w-full flex items-center gap-2.5 py-2 {isRail ? 'justify-center px-0' : 'px-3.5'} hover:bg-base-100/70 transition-colors bg-transparent border-none cursor-pointer text-left"
        title={$t('nav.marketplace')}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class="shrink-0"><path d="M3 9h18v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z"/><path d="M3 9 5 3h14l2 6"/><path d="M9 13h6"/></svg>
        {#if !isRail}<span class="text-sm">{$t('nav.marketplace')}</span>{/if}
      </button>
    </div>
    <UserMenu collapsed={isRail} />
  {/snippet}
</CollapsibleRail>

<!-- Columns 2+3: rendered by child routes -->
{@render children()}

{#if !primaryChristened && !agentsLoading}
  <ChristeningModal
    oncreated={(threadId, name) => {
      primaryChristened = true;
      loadAgentRoster();
      goto(`/assistant/threads/${threadId}`);
    }}
  />
{/if}

{#if newEmployeeOpen}
  <NewEmployeeModal
    onclose={() => (newEmployeeOpen = false)}
    oncreated={(id, _name, threadId) => {
      newEmployeeOpen = false;
      loadAgentRoster();
      // Land where the new employee is INTRODUCING itself, not on an
      // empty composer beside it.
      goto(threadId ? `/${id}/threads/${threadId}` : `/${id}/threads`);
    }}
  />
{/if}

<AgentSettingsModal
  open={settingsSection !== null}
  section={settingsSection ?? 'general'}
  agentName={agent?.name ?? ''}
  avatarInitial={agent?.initial ?? ''}
  avatarClass={agentColor ? `${agentColor.bgClass} ${agentColor.inkClass}` : ''}
  onsection={selectSection}
  onclose={closeSettings}
/>

<ShelfModal
  open={openRunId !== null}
  title={agent ? `${agent.name} — ${$t('agentActivity.runDetail')}` : $t('agentActivity.runDetail')}
  avatarInitial={agent?.initial ?? ''}
  avatarClass={agentColor ? `${agentColor.bgClass} ${agentColor.inkClass}` : ''}
  onclose={closeRun}
>
  <div class="flex-1 min-h-0 flex flex-col overflow-hidden">
    {#if openRunId}<RunDetail runId={openRunId} onclose={closeRun} />{/if}
  </div>
</ShelfModal>

<!-- The shelf lays its children out in a row (settings puts its nav beside its
     content); the storefront stacks, so it owns its own column. -->
<ShelfModal open={marketOpen} title={$t('nav.marketplace')} onclose={closeMarket}>
  <div class="flex-1 min-w-0 min-h-0 flex flex-col">
    <!-- The same top-level sections the /marketplace page offers. They stay put
         over product detail too — a section is always one tap away, and tapping
         one is the way back out of a product. -->
    <div class="flex items-center gap-1 px-4 pt-2 shrink-0 overflow-x-auto">
      {#each MARKET_KINDS as k (k.id)}
        <button
          class="px-3 py-1.5 rounded-field text-sm cursor-pointer border transition-colors whitespace-nowrap {!marketDetail &&
          market.kind === k.id
            ? 'bg-primary/10 border-primary/30 text-base-content font-medium'
            : 'bg-transparent border-transparent text-base-content/60 hover:bg-base-100/70 hover:text-base-content'}"
          onclick={() => selectMarketKind(k.id)}
        >
          {$t(k.labelKey)}
        </button>
      {/each}
    </div>
    <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
    <div class="flex-1 min-h-0 flex" onclickcapture={interceptMarketClick}>
      <!-- Same category rail as the /marketplace page — its plain /marketplace
           hrefs are rerouted into modal state by the capture handler above. -->
      {#if !marketDetail && market.kind !== 'collections' && hasCategoryRail(market.kind)}
        <div class="hidden md:block w-52 shrink-0 border-r border-base-300 bg-base-200 overflow-y-auto">
          <CategoryRail kind={market.kind} activeFilter={market.filter} />
        </div>
      {/if}
      <div class="flex-1 min-w-0 overflow-y-auto">
        {#if marketDetail}
          {#key marketDetail.id}
            <ProductDetail itemId={marketDetail.id} artifactType={marketDetail.type} />
          {/key}
        {:else}
          {#key `${market.kind}|${market.category}|${market.publisher}|${market.price}|${market.filter}`}
            <MarketplaceBrowse
              kind={market.kind}
              price={market.price}
              category={market.category}
              publisher={market.publisher}
              filter={market.filter}
            />
          {/key}
        {/if}
      </div>
    </div>
  </div>
</ShelfModal>

<ShelfModal
  open={runsOpen}
  title={agent ? `${agent.name} — ${$t('nav.runs')}` : $t('nav.runs')}
  avatarInitial={agent?.initial ?? ''}
  avatarClass={agentColor ? `${agentColor.bgClass} ${agentColor.inkClass}` : ''}
  onclose={closeRuns}
>
  <RunsPane onopen={openRun} />
</ShelfModal>

<!-- A workroom: the owner's live seat in a mission room an employee opened. -->
<ShelfModal open={openRoomObj !== null} title={openRoomObj?.name ?? ''} onclose={closeRoom}>
  {#if openRoomObj}
    {#key openRoomObj.channelId}
      <WorkroomView
        room={openRoomObj}
        roster={allAgents.map((a) => ({ id: a.id, name: a.name, initial: a.initial, color: a.color, loopAgentId: a.loopAgentId }))}
      />
    {/key}
  {/if}
</ShelfModal>

<!-- View-only coworker transcript: what one employee told another, verbatim.
     The owner reads; steering happens in the employee's own chat. -->
<ShelfModal
  open={cwKey !== null}
  title={cwNames ? `${cwNames.sender} ⇄ ${cwNames.target}` : ''}
  onclose={closeCoworkerThread}
>
  {#if cwKey}
    {#key cwKey}
      <CoworkerThreadView
        threadKey={cwKey}
        senderName={cwNames?.sender ?? ''}
        targetName={cwNames?.target ?? ''}
      />
    {/key}
  {/if}
</ShelfModal>

<ShelfModal open={inboxOpen} title={$t('nav.inbox')} onclose={closeInbox}>
  <InboxView
    embedded
    selectedId={inboxSelected}
    onselect={selectInboxItem}
    onnavigate={(link) => { closeInbox(); goto(link); }}
  />
</ShelfModal>
