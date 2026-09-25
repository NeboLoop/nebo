<script lang="ts">
  import { openWebBilling } from '$lib/billing';
  import { t } from 'svelte-i18n';
  import { devMode } from '$lib/stores/devmode.js';
  import ChatComposer from './ChatComposer.svelte';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import WorkViewer from './WorkViewer.svelte';
  import DesktopView from './DesktopView.svelte';
  import { teachStart, teachStop, getToolOutput } from '$lib/api/nebo';
  import ShareArtifactModal from './ShareArtifactModal.svelte';
  import AskWidget from './AskWidget.svelte';
  import ConsentChip from './ConsentChip.svelte';
  import type { EmployeeConsentPayload } from './ConsentChip.svelte';
  import type { AskWidgetDef } from './AskWidget.svelte';
  import { renderMentionChips } from '$lib/mentions';
  import { downloadArtifact } from '$lib/chat/download';
  import { backendUrl, backendBase } from '$lib/api/base';
  import { addToast } from '$lib/stores/toast';
  import { parseMarkdown } from '$lib/markdown';
  import FileText from 'lucide-svelte/icons/file-text';
  import MessageSquareLock from 'lucide-svelte/icons/message-square-lock';
  import { page } from '$app/stores';
  import Code from 'lucide-svelte/icons/code';
  import Table from 'lucide-svelte/icons/table';
  import Presentation from 'lucide-svelte/icons/presentation';
  import type { UploadedAttachment } from '$lib/types/attachment';
  import type { SessionGoalStatus } from '$lib/api/neboComponents';
  import { attSrc, stripAttachmentNotes } from '$lib/types/attachment';
  import { flushSync } from 'svelte';
  import type { Snippet } from 'svelte';
  import { getAttachmentType, formatFileSize, attachmentMediaUrl } from '$lib/types/attachment';
  import { NEAR_BOTTOM_PX, distanceFromBottom } from '$lib/chat/scroll';
  import { threadKey } from '$lib/chat/sessionKey';
  import { openAsks } from '$lib/stores/permissionAsks';
  import PermissionAskCard from '$lib/components/PermissionAskCard.svelte';
  import type { HelperLine } from '$lib/chat/helpers';
  import { stepMeta } from '$lib/chat/stepMeta';

  interface Artifact {
    /** Stable container id — same across every version of this document. */
    id: string;
    documentId: string;
    /** 1-based version number of this write. */
    version: number;
    messageId?: string;
    /** Timestamp of the turn that produced this version (provenance). */
    time?: string;
    title: string;
    kind: 'document' | 'code' | 'table' | 'slides';
    url?: string;
    /** Source behind a compiled artifact (.jsx behind .html) — enables the Preview/Code toggle. */
    codeUrl?: string;
  }

  // One tool invocation in an assistant reply's timeline. Tools live ON the reply
  // they belong to (`assistant.tools[]`) — never as sibling messages — so they
  // can't orphan or reorder. Matches the controller's ToolUse + NeboLoop.
  interface SearchResultItem {
    title: string;
    url: string;
    snippet?: string;
  }
  interface SearchResultsPayload {
    kind: 'search_results';
    groups: { query: string; results: SearchResultItem[] }[];
  }
  interface ToolMsg {
    name: string;
    status: string;
    request: Record<string, unknown>;
    response: string;
    statusText?: string;
    label?: string;     // human activity label (gerund), from the start phase
    outcome?: string;   // past-tense outcome, from the result phase
    durationMs?: number;
    /** Structured rendering payload (ToolResult.payload) — known kinds render
     *  as rich cards; unknown kinds are ignored. */
    payload?: { kind: string; [k: string]: unknown };
    /** Live deep-research snapshot (research_progress events). */
    research?: { kind: string; [k: string]: unknown };
    /** The call id — what the server keys a tool's stored output by. */
    toolId?: string;
    /** The stored result was cut to a preview; opening the row fetches the
     *  rest instead of every transcript page carrying every byte. */
    truncated?: boolean;
  }

  type TeamPost = { teamId: string; teamName: string; from: string; fromOwner?: boolean; text: string };
  type Message =
    | { type: 'user'; content: string; time?: string; attachments?: UploadedAttachment[]; pending?: boolean; teamPost?: TeamPost }
    | { type: 'thinking'; content: string; duration: string }
    | { type: 'ask'; requestId: string; prompt: string; widgets: AskWidgetDef[]; response?: string; cancelled?: boolean }
    | { type: 'assistant'; content: string; time?: string; delegateAgentId?: string; delegateAgentName?: string; id?: string; attachments?: UploadedAttachment[]; tools?: ToolMsg[]; streaming?: boolean };

  type AgentInfo = { id: string; name: string; color: string; initial: string; role: string; status: string; isApp?: boolean };

  let { messages = [], agentName = 'Agent', agentId = '', threadId = '', sessionId = '', headerTitle = '', headerRight = '', placeholder = '', emptyIcon = '', emptyTitle = '', emptyDesc = '', allAgents = [], onteachsent, activityStatus = '', helpers = [], tokenUsage = null, goal = null, quotaWarning = '', chatError = '', recapText = '', onsend, onstop, onedit, onredo, onasksubmit, onrestoreversion, ondismisswarning, ondismisserror, onloadmore, isLoading = false, isLoadingMore = false, historyLoading = false, hasMore = false, allowAttachments = true, flowsPane, onopenruns, onsettings, isolated = false, isApp = false, onopenapp, onback, askQueueLength = 0, composerPrefill = '', onprefilled }: {
    messages?: Message[];
    /** Employee-scoped views for the work pane. Omitted on chats with no
     *  employee behind them (channel setup help, the embed), and the matching
     *  header icon then doesn't render. */
    flowsPane?: Snippet;
    /** Runs open as a modal over the workspace, not in the pane. */
    onopenruns?: () => void;
    onsettings?: () => void;
    /** memory.context_isolated — this employee's conversations are sealed. */
    isolated?: boolean;
    /** This employee is an app: badge the header and offer Open App. */
    isApp?: boolean;
    onopenapp?: () => void;
    /** Mobile back-to-list. A real navigation (goto) so the URL changes and
     *  the browser back button stays truthful; rendered only when provided. */
    onback?: () => void;
    /** Questions parked behind the one the owner is looking at. */
    askQueueLength?: number;
    /** Starter text for the composer (prefill, don't send). */
    composerPrefill?: string;
    onprefilled?: () => void;
    agentName?: string;
    agentId?: string;
    threadId?: string;
    sessionId?: string;
    headerTitle?: string;
    headerRight?: string;
    placeholder?: string;
    emptyIcon?: string;
    emptyTitle?: string;
    emptyDesc?: string;
    allAgents?: AgentInfo[];
    activityStatus?: string;
    /** Helpers started from this conversation that are still working. */
    helpers?: HelperLine[];
    tokenUsage?: { input: number; output: number; cacheRead?: number; cacheCreation?: number; overhead?: number } | null;
    /** The thread's agreed goal while it is worked toward; null hides the line. */
    goal?: SessionGoalStatus | null;
    quotaWarning?: string;
    chatError?: string;
    /** The owner recap for the last finished turn (`turn_recap`, WP2.5):
     *  one or two plain sentences for coming back to the thread. Cleared by
     *  the page on thread switch and on send — a stale recap from an
     *  earlier turn never lingers under a newer one. */
    recapText?: string;
    onsend?: (text: string, files: { file: File; id: string; previewUrl: string | null; isImage: boolean }[]) => void;
    onteachsent?: (message: string, sessionKey: string) => void;
    onstop?: () => void;
    onedit?: (msgIndex: number, newContent: string) => void;
    onredo?: (msgIndex: number) => void;
    onasksubmit?: (requestId: string, value: string) => void;
    onrestoreversion?: (documentId: string, version: number) => void;
    ondismisswarning?: () => void;
    ondismisserror?: () => void;
    onloadmore?: () => void;
    isLoading?: boolean;
    /** The thread's transcript is still being fetched — show that, not "empty". */
    historyLoading?: boolean;
    isLoadingMore?: boolean;
    hasMore?: boolean;
    /** Hide the attach affordance when the chat's send pathway ignores files. */
    allowAttachments?: boolean;
  } = $props();

  let composerRef = $state<{ focus: () => void; focusAndInsert: (char: string) => void; addFiles: (files: File[]) => void } | null>(null);
  let creationsOpen = $state(false);
  // The bot's computer takes over the work panel while open.
  type PaneView = 'work' | 'flows';
  // Flows and runs belong to an employee, not to a chat, so they arrive as
  // props from the agent route. Surfaces without an employee (the channels
  // help chat, the embed) pass nothing and the icons don't render.
  let paneView = $state<PaneView>('work');

  // Teach-a-task: record a demonstration on the bot's computer, then hand
  // the artifacts to the agent to study (the normal run does the learning —
  // vision + file + skill tools; a voluntary skill save is the organic
  // learning pathway).
  let teachActive = $state(false);
  let teachError = $state('');
  let teachSeconds = $state(0);
  let teachTimer: ReturnType<typeof setInterval> | null = null;

  async function startTeach() {
    teachError = '';
    try {
      await teachStart();
    } catch (e) {
      teachError = e instanceof Error ? e.message : String(e);
      return;
    }
    computerFull = true;
    teachActive = true;
    teachSeconds = 0;
    teachTimer = setInterval(() => (teachSeconds += 1), 1000);
  }

  async function stopTeach() {
    if (teachTimer) { clearInterval(teachTimer); teachTimer = null; }
    teachActive = false;
    try {
      // The display layer only reports the event (who + where). The backend
      // owns the visible message AND the steering briefing (mention_context
      // system-reminder) and dispatches the learning run itself; it returns
      // the persisted message text so we can echo it without a second copy.
      const res = await teachStop({
        agentId,
        sessionKey: sessionId || (threadId ? threadKey(agentId, threadId) : ''),
      });
      onteachsent?.(res.message ?? '', res.sessionKey ?? '');
    } catch (e) {
      teachError = e instanceof Error ? e.message : String(e);
    }
  }

  function fmtTeach(sec: number): string {
    const m = Math.floor(sec / 60);
    const s2 = (sec % 60).toString().padStart(2, '0');
    return `${m}:${s2}`;
  }
  // Empty = default panel title ($t('chat.work') at render time).
  let creationsTitle = $state('');
  let activeArtifactId = $state<string | null>(null);
  // Pinned version of the active document; null = follow the latest version.
  let activeVersion = $state<number | null>(null);

  // Render assistant message content with basic markdown + mention chips.
  // Code blocks get a copy affordance: each <pre> is wrapped with a positioned
  // button handled by delegated click (copyCodeBlock) — the button copies the
  // wrapped <code>'s text, so no payload attributes are needed.
  function renderMarkdown(content: string): string {
    if (!content) return '';
    const html = parseMarkdown(content);
    const withCopy = html
      .replace(
        /<pre>/g,
        `<div class="relative group/code"><button type="button" data-code-copy title="${$t('chat.copyCode')}" class="absolute top-2 right-2 z-10 px-2 py-0.5 rounded text-xs font-medium bg-base-100/80 border border-base-content/10 text-base-content/60 opacity-0 group-hover/code:opacity-100 hover:text-base-content hover:bg-base-200 cursor-pointer transition-opacity">${$t('common.copy')}</button><pre>`
      )
      .replace(/<\/pre>/g, '</pre></div>');
    return renderMentionChips(withCopy, allAgents);
  }

  // Delegated handler for the injected code-block copy buttons.
  function copyCodeBlock(target: HTMLElement): boolean {
    const btn = target.closest?.('[data-code-copy]') as HTMLElement | null;
    if (!btn) return false;
    const code = btn.parentElement?.querySelector('pre code, pre')?.textContent ?? '';
    navigator.clipboard.writeText(code).then(() => {
      const prev = btn.textContent;
      btn.textContent = $t('chat.copied');
      setTimeout(() => { btn.textContent = prev; }, 1200);
    });
    return true;
  }

  // "Work" artifacts produced by the agent — flattened from each assistant message's
  // workItems (set by the controller from run-produced document URLs), tagged with messageId.
  const artifacts = $derived<Artifact[]>(
    (messages as any[]).flatMap((m) =>
      (m.workItems ?? []).map((w: any) => ({
        id: w.documentId ?? w.id, documentId: w.documentId ?? w.id, version: w.version ?? 1,
        messageId: m.id, time: m.time, title: w.title, kind: w.kind, url: w.url, codeUrl: w.codeUrl,
      }))
    )
  );

  // Group versions per document container (oldest → newest), deduped by version.
  const documentVersions = $derived.by(() => {
    const map = new Map<string, Artifact[]>();
    for (const a of artifacts) {
      const list = map.get(a.documentId) ?? [];
      const existing = list.findIndex((v) => v.version === a.version);
      if (existing >= 0) list[existing] = a; else list.push(a);
      map.set(a.documentId, list);
    }
    for (const list of map.values()) list.sort((x, y) => x.version - y.version);
    return map;
  });
  // Distinct documents, represented by their latest version.
  const documents = $derived<Artifact[]>(
    [...documentVersions.values()].map((vs) => vs[vs.length - 1])
  );
  // Versions of the currently-open document (for the version dropdown + badge).
  const activeVersionList = $derived<Artifact[]>(
    documentVersions.get(activeArtifactId ?? '') ?? []
  );

  const artifactIcons = { document: FileText, code: Code, table: Table, slides: Presentation };
  // The shown artifact = the pinned version (activeVersion) or, by default, the
  // latest — so a new version produced by the AI refreshes the open viewer in place.
  const activeArtifact = $derived.by(() => {
    if (activeVersionList.length === 0) return undefined;
    if (activeVersion != null) {
      return activeVersionList.find((v) => v.version === activeVersion) ?? activeVersionList[activeVersionList.length - 1];
    }
    return activeVersionList[activeVersionList.length - 1];
  });

  // Turn an inline `filename` mention (rendered as <code>filename</code>) into a clickable
  // chip when that filename is one of the message's produced Work items.
  function linkWorkMentions(html: string, items?: { id: string; title: string }[]): string {
    if (!items?.length) return html;
    let out = html;
    for (const it of items) {
      const code = `<code>${it.title}</code>`;
      if (!out.includes(code)) continue;
      const chip = `<button type="button" data-work-id="${it.id.replace(/"/g, '&quot;')}" class="inline-flex items-center px-1.5 py-0.5 rounded-md bg-base-200 border border-base-content/10 hover:border-primary/40 hover:bg-primary/5 cursor-pointer text-xs font-mono no-underline text-base-content align-baseline">${it.title}</button>`;
      out = out.split(code).join(chip);
    }
    return out;
  }

  // Full-size image viewer (lightbox) — opens images IN the app instead of an
  // external browser window (Tauri opens <a target="_blank"> in the system browser).
  let lightboxUrl = $state<string | null>(null);

  function handleWorkMentionClick(e: MouseEvent) {
    if (copyCodeBlock(e.target as HTMLElement)) {
      e.preventDefault();
      return;
    }
    const t = e.target as HTMLElement;
    // Markdown screenshots/images → open in the in-app lightbox, never external.
    if (t?.tagName === 'IMG' && (t as HTMLImageElement).src) {
      e.preventDefault();
      lightboxUrl = (t as HTMLImageElement).src;
      return;
    }
    const link = t?.closest?.('a') as HTMLAnchorElement | null;
    if (link?.href && /\.(png|jpe?g|gif|webp|svg|bmp)(\?|#|$)/i.test(link.href)) {
      e.preventDefault();
      lightboxUrl = link.href;
      return;
    }
    const el = t?.closest?.('[data-work-id]');
    if (el) {
      e.preventDefault();
      openArtifact(el.getAttribute('data-work-id') || '');
    }
  }

  // Preview ↔ Code toggle for the active artifact (compiled artifacts pair
  // their source via codeUrl; plain html shows its own markup).
  let viewSource = $state(false);

  // Share dialog for the active artifact (loop channels / members).
  let shareOpen = $state(false);

  // Text-like formats copy their content; binaries copy the file's URL.
  const COPYABLE_EXTS = ['md', 'txt', 'html', 'htm', 'csv', 'tsv', 'json', 'js', 'mjs', 'cjs',
    'ts', 'tsx', 'jsx', 'py', 'rs', 'go', 'sh', 'bash', 'css', 'yaml', 'yml', 'toml', 'sql',
    'svelte', 'rb', 'java', 'c', 'h', 'cpp', 'xml', 'markdown', 'log'];

  async function copyArtifact() {
    if (!activeArtifact?.url) return;
    const src = backendUrl(activeArtifact.url);
    const ext = (activeArtifact.title.split('.').pop() || '').toLowerCase();
    try {
      if (COPYABLE_EXTS.includes(ext)) {
        // iOS Safari revokes clipboard access once the user gesture crosses an
        // await — hand the clipboard a promise synchronously (ClipboardItem)
        // so the write stays inside the gesture; writeText fallback elsewhere.
        const textPromise = fetch(src).then(async (res) => {
          if (!res.ok) throw new Error(`${res.status}`);
          return res.text();
        });
        if (typeof ClipboardItem !== 'undefined' && navigator.clipboard.write) {
          await navigator.clipboard.write([
            new ClipboardItem({
              'text/plain': textPromise.then((t) => new Blob([t], { type: 'text/plain' }))
            })
          ]);
        } else {
          await navigator.clipboard.writeText(await textPromise);
        }
      } else {
        await navigator.clipboard.writeText(new URL(src, window.location.origin).href);
      }
      addToast($t('chat.copied'), 'success');
    } catch {
      addToast($t('chat.copyFailed'), 'error');
    }
  }

  function openArtifact(id: string) {
    // Open first: openPane clears a selection left over from another thread,
    // so setting the new id afterwards can't be undone by it.
    openPane('work');
    activeArtifactId = id;
    activeVersion = null; // follow latest; the version dropdown pins an older one
    viewSource = false;
    const a = artifacts.find(x => x.documentId === id);
    if (a) creationsTitle = a.title;
    // WorkViewer owns fetching + rendering (text/binary/media per format).
    // Opening the panel narrows the chat column and reflows the transcript —
    // re-pin to the bottom so the message you clicked from stays in view.
    requestAnimationFrame(() => scrollToBottom());
  }
  const CREATIONS_MIN = 220;
  // The chat column must stay usable no matter how wide the panel goes —
  // wide enough for the composer, message bubbles, and header controls.
  const CHAT_MIN = 400;
  let creationsWidth = $state(CREATIONS_MIN);
  // The split is stored as a FRACTION of the container (default half) so a
  // window resize keeps the proportion instead of letting the flex chat
  // column absorb the whole delta. Dragging updates the fraction.
  let creationsFraction = $state(0.5);
  let userResized = $state(false);
  let resizing = $state(false);
  let workFull = $state(false);
  let containerEl = $state<HTMLDivElement | null>(null);

  // One clamp for every pathway that sets the panel width (open, drag,
  // container resize): never below CREATIONS_MIN, never so wide the chat
  // column drops under CHAT_MIN.
  function clampPanelWidth(w: number): number {
    if (!containerEl) return Math.max(CREATIONS_MIN, w);
    const total = containerEl.getBoundingClientRect().width;
    const max = Math.max(CREATIONS_MIN, total - CHAT_MIN);
    return Math.max(CREATIONS_MIN, Math.min(max, w));
  }

  // Re-clamp when the container shrinks (window resize, sidebar toggle) —
  // the panel width is absolute px, so without this the flex chat column
  // absorbs the entire loss and collapses.
  $effect(() => {
    if (!containerEl) return;
    const ro = new ResizeObserver(() => {
      if (!creationsOpen || !containerEl) return;
      const total = containerEl.getBoundingClientRect().width;
      creationsWidth = clampPanelWidth(total * creationsFraction);
    });
    ro.observe(containerEl);
    return () => ro.disconnect();
  });

  // The ONE way the pane opens. Every caller goes through here — Work, Computer
  // and teach-a-task all used to set the flags themselves, so two of the three
  // opened at whatever stale width was left over instead of sizing themselves.
  // Default is half the chat area, unless the user has dragged it to their own
  // width this session; always resizable after.
  // containerEl wraps the chat column AND the pane, so its width doesn't change
  // when the pane opens — half of it is half either way.
  function openPane(view: PaneView) {
    paneView = view;
    creationsOpen = true;
    // Opening Work with nothing selected shows the artifact list — never
    // auto-pick a file the user didn't ask for.
    if (view === 'work' && activeArtifactId && !documentVersions.has(activeArtifactId)) {
      activeArtifactId = null; // stale selection from another thread
      activeVersion = null;
    }
    if (!userResized && containerEl) {
      creationsFraction = 0.5;
      creationsWidth = clampPanelWidth(containerEl.getBoundingClientRect().width * creationsFraction);
    }
  }

  function closePane() {
    creationsOpen = false;
    workFull = false;
  }

  /** Toggle a pane view from the header: same view closes, different switches. */
  function togglePane(view: PaneView) {
    if (creationsOpen && paneView === view) closePane();
    else openPane(view);
  }

  // The computer takes the whole window rather than a 450px rail. It is the one
  // surface you drive rather than refer to, and a remote desktop scaled into a
  // side panel is unusable.
  let computerFull = $state(false);

  function startResize(e: MouseEvent) {
    e.preventDefault();
    resizing = true;
    const onMove = (ev: MouseEvent) => {
      if (!containerEl) return;
      const rect = containerEl.getBoundingClientRect();
      creationsWidth = clampPanelWidth(rect.right - ev.clientX);
      if (rect.width > 0) creationsFraction = creationsWidth / rect.width;
      userResized = true;
    };
    const onUp = () => {
      resizing = false;
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  }

  // Edit state
  let editingIdx = $state<number | null>(null);
  let editText = $state('');
  let editTextareaEl = $state<HTMLTextAreaElement | null>(null);

  // Clipboard feedback
  let copiedIdx = $state<number | null>(null);
  let copiedTimeout: ReturnType<typeof setTimeout> | null = null;

  function copyMessage(content: string, idx: number) {
    navigator.clipboard.writeText(content);
    if (copiedTimeout) clearTimeout(copiedTimeout);
    copiedIdx = idx;
    copiedTimeout = setTimeout(() => { copiedIdx = null; }, 1500);
  }


  /** Hostname for a search-result row (favicon + domain column). */
  function resultHost(url: string): string {
    try {
      return new URL(url).hostname.replace(/^www\./, '');
    } catch {
      return url;
    }
  }
  interface ResearchState {
    question?: string;
    depth?: string;
    angles?: string[];
    phase?: string;
    results_found?: number;
    sources_read?: number;
    domains?: Record<string, number>;
    claims_verified?: number;
    started_ms?: number;
    elapsed_ms?: number;
    complete?: boolean;
  }
  /** Live snapshot while running; final summary payload afterwards. */
  function researchState(tool: ToolMsg): ResearchState | null {
    if (tool.payload?.kind === 'research_summary') {
      return { ...(tool.payload as ResearchState), complete: true, phase: 'complete' };
    }
    if (tool.research && typeof tool.research === 'object') {
      return tool.research as ResearchState;
    }
    return null;
  }
  // 1s tick drives the live elapsed timer on running research cards.
  let clockNow = $state(Date.now());
  $effect(() => {
    const t = setInterval(() => (clockNow = Date.now()), 1000);
    return () => clearInterval(t);
  });
  function fmtElapsed(ms: number): string {
    const s = Math.max(0, Math.floor(ms / 1000));
    const m = Math.floor(s / 60);
    return m > 0 ? `${m}m ${s % 60}s` : `${s}s`;
  }
  function researchPhaseLabel(phase?: string): string {
    switch (phase) {
      case 'scoping': return $t('chat.researchScoping');
      case 'searching': return $t('chat.researchSearching');
      case 'reading': return $t('chat.researchReading');
      case 'verifying': return $t('chat.researchVerifying');
      case 'writing': return $t('chat.researchWriting');
      default: return '';
    }
  }
  function topDomains(domains?: Record<string, number>): [string, number][] {
    return Object.entries(domains ?? {}).sort((a, b) => b[1] - a[1]).slice(0, 4);
  }
  function otherDomainCount(domains?: Record<string, number>): number {
    const entries = Object.entries(domains ?? {});
    return Math.max(0, entries.length - 4);
  }

  // A run receipt: the run narrator's projection attached by the work tool
  // when a finished run's status is fetched. A card is a receipt, not a
  // decoration — every line is recorded state, and the card deep-links into
  // the narrated run detail.
  interface RunReceiptPayload {
    kind: 'run_receipt';
    runId: string;
    workflow?: string;
    status?: string;
    error?: string | null;
    errorActivity?: string | null;
    order?: string[];
    display?: {
      input?: { line?: string | null; facts?: { key: string; value: string }[] } | null;
      activities?: Record<string, { line?: string | null; verdict?: string }>;
    };
    [k: string]: unknown;
  }
  function runReceipt(tool: ToolMsg): RunReceiptPayload | null {
    return tool.payload?.kind === 'run_receipt'
      ? (tool.payload as unknown as RunReceiptPayload)
      : null;
  }
  function receiptLines(rr: RunReceiptPayload): { line: string; failed: boolean; stopped: boolean }[] {
    const acts = rr.display?.activities ?? {};
    const ids = rr.order?.length ? rr.order : Object.keys(acts);
    return ids
      .map((id) => {
        const a = acts[id];
        return {
          line: a?.line || id,
          failed: rr.errorActivity === id,
          stopped: a?.verdict === 'stopped',
        };
      })
      .filter((l) => l.line);
  }

  function searchPayload(tool: ToolMsg): SearchResultsPayload | null {
    return tool.payload?.kind === 'search_results' && Array.isArray((tool.payload as unknown as SearchResultsPayload).groups)
      ? (tool.payload as unknown as SearchResultsPayload)
      : null;
  }

  // Coworker sends are events the owner reads ("Messaged Search Analyst"),
  // never plumbing inside the collapsed tool group.
  interface CoworkerEventPayload {
    kind: 'coworker_message';
    to?: string;
    toAgentId?: string;
    threadKey?: string;
    text?: string;
    reply?: string | null;
    [k: string]: unknown;
  }
  function coworkerEvents(tools: ToolMsg[] | undefined): CoworkerEventPayload[] {
    return (tools ?? [])
      .flatMap((t) => (t.payload?.kind === 'coworker_message' ? [t.payload as CoworkerEventPayload] : []));
  }
  // A drafted employee's consent line is the owner's to answer, never
  // plumbing inside the collapsed tool group.
  function consentLines(tools: ToolMsg[] | undefined): EmployeeConsentPayload[] {
    return (tools ?? [])
      .flatMap((t) => (t.payload?.kind === 'employee_consent' ? [t.payload as EmployeeConsentPayload] : []));
  }
  function nonCoworkerTools(tools: ToolMsg[] | undefined): ToolMsg[] {
    return (tools ?? []).filter((t) => t.payload?.kind !== 'coworker_message');
  }
  // The event chip deep-links to the view-only employee↔employee transcript
  // (?cw=<threadKey> — URL state, same as every other shell surface). The
  // sender's display name rides as ?cwf= because the thread key's context
  // segment is the MATTER id for isolated senders — unparseable to a name.
  // The chip lives in the sender's own chat, so agentName IS the sender.
  function cwHref(key: string): string {
    const url = new URL($page.url);
    url.searchParams.set('cw', key);
    if (agentName) url.searchParams.set('cwf', agentName);
    return url.pathname + url.search;
  }

  function startEdit(idx: number, content: string) {
    editingIdx = idx;
    editText = content;
    requestAnimationFrame(() => {
      if (editTextareaEl) {
        editTextareaEl.style.height = 'auto';
        editTextareaEl.style.height = editTextareaEl.scrollHeight + 'px';
        editTextareaEl.focus();
        editTextareaEl.selectionStart = editTextareaEl.value.length;
      }
    });
  }

  function cancelEdit() {
    editingIdx = null;
    editText = '';
  }

  function saveEdit(idx: number) {
    const val = editText.trim();
    if (!val) return;
    onedit?.(idx, val);
    editingIdx = null;
    editText = '';
  }

  // The large-input pipeline replaces a huge pasted prompt with a pointer +
  // summary FOR THE MODEL — but that replacement was stored as the user's
  // message, so the transcript showed internal plumbing ("can be read with
  // os(resource: ...)"). Render it as a clean note + the summary instead.
  // Attachment pointer notes ("[Attached: x.md (13 KB) — saved at /path...]",
  // audio variants) are appended to the prompt FOR THE MODEL — the transcript
  // already renders the real attachment chips, so the pointer text is
  // plumbing duplicated into the human view. Strip it from display only;
  // the stored content (the model's context) is untouched.

  const LARGE_INPUT_RE = /^\[This message contained a large [\s\S]*?\((\d+) characters[\s\S]*?Here is a summary:\]\s*/;
  function parseLargeInput(content: string): { chars: string; summary: string } | null {
    const m = content.match(LARGE_INPUT_RE);
    if (!m) return null;
    return { chars: Number(m[1]).toLocaleString(), summary: content.slice(m[0].length) };
  }

  function handleEditKeydown(e: KeyboardEvent, idx: number) {
    if (e.key === 'Escape') {
      e.preventDefault();
      cancelEdit();
    }
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      saveEdit(idx);
    }
  }

  function handleEditInput() {
    if (editTextareaEl) {
      editTextareaEl.style.height = 'auto';
      editTextareaEl.style.height = editTextareaEl.scrollHeight + 'px';
    }
  }

  function redoMessage(idx: number) {
    onredo?.(idx);
  }

  export function focusComposer() {
    composerRef?.focus();
  }

  // Auto-focus chat input when user starts typing anywhere
  function handleGlobalKeydown(e: KeyboardEvent) {
    // Esc anywhere in the chat stops the running turn, the way it does in
    // Claude Code. The composer handles its own Esc while it has focus.
    if (e.key === 'Escape' && isLoading && !(document.activeElement as HTMLElement)?.isContentEditable
      && !document.querySelector('[data-modal-open]')) {
      e.preventDefault();
      onstop?.();
      return;
    }
    if (document.activeElement?.tagName === 'INPUT' || document.activeElement?.tagName === 'TEXTAREA') return;
    if ((document.activeElement as HTMLElement)?.isContentEditable) return;
    if (e.ctrlKey || e.metaKey || e.altKey || e.key.length > 1) return;
    if (document.querySelector('[data-modal-open]')) return;
    e.preventDefault();
    composerRef?.focusAndInsert(e.key);
  }

  export function showCreations(title = '') {
    creationsTitle = title;
    openPane('work');
  }

  export function hideCreations() {
    closePane();
  }

  const hasMessages = $derived(messages.length > 0);

  // Scroll state
  let messagesContainer = $state<HTMLDivElement | null>(null);
  let messagesContent = $state<HTMLDivElement | null>(null);
  let showScrollButton = $state(false);
  let autoScrollEnabled = $state(true);
  let scrollingProgrammatically = false;
  let programmaticUntil = 0;
  let settleRAF: number | null = null;
  let initialScrollDone = false;
  let prevScrollHeight = 0;
  let lastScrollTop = 0;
  // Reserved room for the streaming reply (the claude.ai turn model): on send,
  // the user's message pins to the TOP of the viewport and a trailing spacer
  // reserves the rest of it. The reply streams INTO the reserved room — the
  // spacer shrinks 1:1 with content growth, total scroll height stays constant,
  // and the view is perfectly calm. When the room runs out (spacer hits 0),
  // normal follow-the-stream pinning takes over.
  let turnSpacerHeight = $state(0);
  /// Identity of the latest user message (grouped messages carry no id — key on
  /// position + content so both new sends and edit-resubmits re-arm the room).
  let lastUserMsgKey: string | null = null;
  /// Identity of the FIRST user message — the prepend detector (see the
  /// turn effect): older history changes it, a new send never does.
  let lastFirstUserKey: string | null = null;
  const TURN_TOP_PAD = 18; // matches the scroller's vertical padding

  /// Recompute the spacer so (last user msg → end of content + spacer) fills
  /// exactly one viewport. Returns the room left for the reply.
  function updateTurnSpacer(): number {
    const scroller = messagesContainer;
    const content = messagesContent;
    if (!scroller || !content || lastUserMsgKey === null) return 0;
    const userEls = content.querySelectorAll<HTMLElement>('[data-user-msg]');
    const target = userEls[userEls.length - 1];
    if (!target) {
      turnSpacerHeight = 0;
      return 0;
    }
    const usedByTurn = content.getBoundingClientRect().bottom - target.getBoundingClientRect().top;
    const room = Math.max(0, Math.round(scroller.clientHeight - usedByTurn - TURN_TOP_PAD * 2));
    turnSpacerHeight = room;
    return room;
  }

  // A new user message: reserve the room and pin their message to the top.
  $effect(() => {
    const users = groupedMessages.filter((m) => m.type === 'user');
    const last = users[users.length - 1];
    const first = users[0];
    const key = last ? `${users.length}:${last.content}` : null;
    const firstKey = first ? `${first.time ?? ''}:${first.content}` : null;
    if (key === lastUserMsgKey || key === null) return;
    // Loading OLDER messages also grows the count and changes the key, but a
    // prepend changes the FIRST user message while a send never does. Treat
    // it as history arriving, not a new turn — the old check pinned the last
    // user message to the top on every load-older, yanking the reader from
    // the top of the transcript back to the bottom (and making the start of a
    // long conversation unreachable).
    const prepended = firstKey !== lastFirstUserKey && lastFirstUserKey !== null;
    lastUserMsgKey = key;
    lastFirstUserKey = firstKey;
    if (prepended) return;
    if (!initialScrollDone) return; // opening an old chat is not a send
    requestAnimationFrame(() => {
      const scroller = messagesContainer;
      const content = messagesContent;
      if (!scroller || !content) return;
      updateTurnSpacer();
      // The spacer is state; it has to be in the DOM before the scroll below
      // or the browser clamps the scroll to the old height and the message
      // lands wherever the previous reply left it, the reply then streaming
      // below the fold while the spacer keeps the follow pin from firing.
      flushSync();
      const userEls = content.querySelectorAll<HTMLElement>('[data-user-msg]');
      const target = userEls[userEls.length - 1];
      if (!target) return;
      scrollingProgrammatically = true;
      scroller.scrollTop +=
        target.getBoundingClientRect().top - scroller.getBoundingClientRect().top - TURN_TOP_PAD;
      lastScrollTop = scroller.scrollTop;
      autoScrollEnabled = true;
      requestAnimationFrame(() => { scrollingProgrammatically = false; });
    });
  });

  function isProgrammaticScroll(): boolean {
    return scrollingProgrammatically || performance.now() < programmaticUntil;
  }

  /** Pin to bottom. Instant for auto-follow (avoids smooth-scroll race);
   *  smooth only for the explicit button. Keep the programmatic lock until
   *  we are near the bottom or the deadline elapses. */
  function pinToBottom(smooth = false) {
    if (!messagesContainer) return;
    scrollingProgrammatically = true;
    programmaticUntil = performance.now() + (smooth ? 600 : 100);
    if (smooth) {
      messagesContainer.scrollTo({ top: messagesContainer.scrollHeight, behavior: 'smooth' });
    } else {
      messagesContainer.scrollTop = messagesContainer.scrollHeight;
    }
    showScrollButton = false;
    autoScrollEnabled = true;
    if (settleRAF) cancelAnimationFrame(settleRAF);
    const settle = () => {
      settleRAF = null;
      if (!messagesContainer) {
        scrollingProgrammatically = false;
        programmaticUntil = 0;
        return;
      }
      const near = distanceFromBottom(messagesContainer) <= NEAR_BOTTOM_PX;
      if (near || performance.now() >= programmaticUntil) {
        scrollingProgrammatically = false;
        programmaticUntil = 0;
        return;
      }
      settleRAF = requestAnimationFrame(settle);
    };
    settleRAF = requestAnimationFrame(settle);
  }

  // Preserve scroll position after older messages are prepended. The scroller
  // opts out of the browser's own scroll anchoring (`.chat-scroller` in
  // app.css): WebKit anchors too now, and the two together moved the view by
  // the prepended height twice — a page past where the reader was.
  $effect(() => {
    if (isLoadingMore && messagesContainer) {
      prevScrollHeight = messagesContainer.scrollHeight;
    }
  });
  $effect(() => {
    // When loading finishes and messages have been prepended, adjust scroll
    if (!isLoadingMore && prevScrollHeight > 0 && messagesContainer) {
      scrollingProgrammatically = true;
      programmaticUntil = performance.now() + 100;
      requestAnimationFrame(() => {
        if (messagesContainer) {
          const added = messagesContainer.scrollHeight - prevScrollHeight;
          messagesContainer.scrollTop += added;
        }
        prevScrollHeight = 0;
        requestAnimationFrame(() => {
          scrollingProgrammatically = false;
          programmaticUntil = 0;
        });
      });
    }
  });

  // Auto-scroll: pin to the bottom whenever the CONTENT grows, not when the
  // message count changes. Streaming appends tokens to the LAST message (count
  // constant), and markdown/images/tool blocks grow after insert — a
  // count-keyed effect misses all of it, which is exactly the "messages stop
  // following the stream" bug. A ResizeObserver on the inner content sees every
  // growth source. Pin INSTANTLY (no smooth): a gliding scroll outlives the
  // programmatic flag and its intermediate positions read as "user scrolled
  // away", disabling auto-scroll mid-stream.
  $effect(() => {
    if (!messagesContent) return;
    const ro = new ResizeObserver(() => {
      const el = messagesContainer;
      if (!el || !autoScrollEnabled || !initialScrollDone) return;
      // While the reply still fits in the reserved room, shrink the spacer to
      // absorb the growth — total height constant, view stays put (calm fill).
      if (updateTurnSpacer() > 0) return;
      scrollingProgrammatically = true;
      programmaticUntil = performance.now() + 100;
      el.scrollTop = el.scrollHeight;
      lastScrollTop = el.scrollTop;
      requestAnimationFrame(() => { scrollingProgrammatically = false; });
    });
    ro.observe(messagesContent);
    return () => ro.disconnect();
  });

  // Initial scroll to bottom. Markdown, tool blocks, and images render
  // asynchronously and keep growing the content AFTER first paint — a single
  // scroll lands mid-conversation. Re-pin to the bottom every frame until the
  // content height stabilizes (or a short cap), so we settle at the true end.
  $effect(() => {
    if (messagesContainer && hasMessages && !initialScrollDone) {
      scrollingProgrammatically = true;
      programmaticUntil = performance.now() + 800;
      let lastHeight = -1;
      let stableFrames = 0;
      let frames = 0;
      const pin = () => {
        const el = messagesContainer;
        if (!el) return;
        el.scrollTop = el.scrollHeight;
        frames += 1;
        if (el.scrollHeight === lastHeight) {
          stableFrames += 1;
        } else {
          stableFrames = 0;
          lastHeight = el.scrollHeight;
        }
        // Settle once the height has held steady for a few frames, or after a
        // ~0.7s cap (guards against content that never stops changing).
        if (stableFrames >= 3 || frames >= 40) {
          showScrollButton = false;
          autoScrollEnabled = true;
          initialScrollDone = true;
          requestAnimationFrame(() => {
            scrollingProgrammatically = false;
            programmaticUntil = 0;
          });
          return;
        }
        requestAnimationFrame(pin);
      };
      requestAnimationFrame(pin);
    }
  });

  // Follow ("sticky") is an INTENT bit, per the reference ScrollBox model:
  // set by send / the scroll button / arriving at the bottom, cleared ONLY by
  // explicit user input (wheel up, touch drag) — NEVER inferred from scroll
  // events. Scroll events can come from our own programmatic pins arriving a
  // frame late, and inferring intent from them is exactly the race that broke
  // send-follow in 0.12.7. Geometry may re-engage follow (a false positive
  // just resumes following at the bottom, which is what the user wants);
  // geometry must never disengage it.
  let touchActive = false;

  function handleWheel(e: WheelEvent) {
    if (e.deltaY < 0) autoScrollEnabled = false; // user is looking up
  }
  function handleTouchStart() {
    touchActive = true;
  }
  function handleTouchEnd() {
    touchActive = false;
  }

  function handleScroll() {
    if (!messagesContainer) return;
    const { scrollTop, scrollHeight, clientHeight } = messagesContainer;
    const dist = scrollHeight - scrollTop - clientHeight;
    const scrolledUp = scrollTop < lastScrollTop - 1;
    lastScrollTop = scrollTop;
    showScrollButton = dist > NEAR_BOTTOM_PX;

    // Any non-programmatic upward scroll is user intent — touch AND wheel.
    // Wheel-up used to leave follow engaged, so every stream chunk yanked
    // the reader back to the bottom mid-scroll ("can't scroll up in an
    // active run"). The programmatic-scroll guard keeps turn-positioning
    // and prepend-anchoring from tripping this.
    if ((touchActive || !isProgrammaticScroll()) && scrolledUp && dist > NEAR_BOTTOM_PX) {
      autoScrollEnabled = false;
    } else if (dist <= NEAR_BOTTOM_PX) {
      autoScrollEnabled = true;
    }

    // Load older messages a screen before the top, so the page is in place
    // before the reader reaches it.
    if (!isProgrammaticScroll() && scrollTop < clientHeight && hasMore && !isLoadingMore && onloadmore) {
      onloadmore();
    }
  }

  function scrollToBottom() {
    pinToBottom(true);
  }

  /** Re-engage follow on send. Positioning is OWNED by the turn-model effect
   *  (user message to top + reserved reply room) — pinning to bottom here
   *  raced it: two programmatic scrolls in one frame window, and the pin's
   *  settle loop outlived its suppression flag, so the turn-scroll's events
   *  read as user movement and killed follow (the 0.12.7 no-scroll bug). */
  function handleSend(
    text: string,
    files: { file: File; id: string; previewUrl: string | null; isImage: boolean }[],
    ...rest: unknown[]
  ) {
    autoScrollEnabled = true;
    showScrollButton = false;
    (onsend as ((t: string, f: typeof files, ...r: unknown[]) => void) | undefined)?.(text, files, ...rest);
  }

  // Dropzone state
  let isDragging = $state(false);
  let dragCounter = $state(0);

  // Tool timeline collapse state, keyed by the owning reply's id (stable across
  // re-renders — index keys would drift as new messages stream in).
  // ── The activity panel: one per assistant turn ──────────────────────────
  // A turn is every assistant segment between two user messages. Everything
  // the model did before its answer — the one-line note it wrote before each
  // tool call, and the tool calls themselves — is one panel, folded under a
  // summary line ("Searched the web, read a page, ran a command"); the answer
  // is the last segment's text and stands alone below it. Rows open on click.
  type AssistantMsg = Extract<Message, { type: 'assistant' }>;
  type ActivityStep =
    | { kind: 'note'; key: string; lines: string[] }
    | { kind: 'tool'; key: string; tool: ToolMsg };

  /** Open state per turn; unset means folded. A live turn stays folded too —
   *  the summary line shimmers while the work happens, and the work is read
   *  by whoever opens it. */
  let activityOpen = $state<Record<string, boolean>>({});

  function turnSegments(idx: number): AssistantMsg[] {
    const out: AssistantMsg[] = [];
    for (let i = idx; i < groupedMessages.length; i++) {
      const m = groupedMessages[i];
      if (m.type !== 'assistant') break;
      out.push(m);
    }
    return out;
  }
  /** The text of a segment that ran tools is a note about the next step; the
   *  text of the segment that ran none is the answer. Consecutive notes fold
   *  into one row whose label is the latest. */
  function activitySteps(segs: AssistantMsg[], keyId: string): ActivityStep[] {
    const steps: ActivityStep[] = [];
    segs.forEach((seg, si) => {
      const tools = shownTools(seg.tools);
      const isAnswer = si === segs.length - 1 && nonCoworkerTools(seg.tools).length === 0;
      const note = seg.content?.trim();
      if (note && !isAnswer) {
        const prev = steps[steps.length - 1];
        if (prev?.kind === 'note') prev.lines.push(note);
        else steps.push({ kind: 'note', key: `${keyId}-n${si}`, lines: [note] });
      }
      tools.forEach((tool, ti) => steps.push({ kind: 'tool', key: `${keyId}-${si}-${ti}`, tool }));
    });
    return steps;
  }
  /** The calls a user should see. A call that failed and was retried is the
   *  employee's business — nothing the user can act on — so it is left out,
   *  except in developer mode, where someone is debugging. */
  function shownTools(tools: ToolMsg[] | undefined): ToolMsg[] {
    const all = nonCoworkerTools(tools);
    return $devMode ? all : all.filter((t) => t.status !== 'error');
  }
  function turnAnswer(segs: AssistantMsg[]): string {
    const last = segs[segs.length - 1];
    return last && nonCoworkerTools(last.tools).length === 0 ? last.content : '';
  }
  /** A note's row shows plain words; markdown marks are for the answer. */
  function plainNote(line: string): string {
    return line.replace(/[*_`#>]+/g, '').replace(/\s+/g, ' ').trim();
  }
  /** The command a call ran, shown as code under its row. */
  function shellCommand(tool: ToolMsg): string {
    const r = (tool.request ?? {}) as Record<string, unknown>;
    return typeof r.command === 'string' ? r.command : '';
  }
  function canExpand(tool: ToolMsg): boolean {
    if (tool.status === 'running') return false;
    return !!(researchState(tool) || runReceipt(tool) || searchPayload(tool) || tool.response || Object.keys(tool.request ?? {}).length);
  }

  // Individual tool result expand state
  let expandedResults = $state<Record<string, boolean>>({});
  // Full outputs fetched for rows whose stored result was cut to a preview,
  // by tool call id. The list ships previews so a tool-heavy thread stays a
  // readable page; the whole thing arrives when someone opens the row.
  let fullOutputs = $state<Record<string, string>>({});
  const outputChatId = $derived(threadId || sessionId);
  const chatSessionKey = $derived(sessionId || (threadId ? threadKey(agentId, threadId) : ''));
  const chatAsks = $derived(chatSessionKey ? $openAsks.filter((a) => a.sessionKey === chatSessionKey) : []);
  async function toggleResult(key: string, tool?: ToolMsg) {
    const opening = !expandedResults[key];
    expandedResults[key] = opening;
    if (!opening || !tool?.truncated || !tool.toolId || !outputChatId) return;
    if (fullOutputs[tool.toolId] !== undefined) return;
    // Hold the preview under the id: it is what stays on screen if the fetch
    // fails, and it keeps a second open from firing the same request.
    fullOutputs[tool.toolId] = tool.response;
    try {
      const full = await getToolOutput(outputChatId, tool.toolId);
      if (full?.output) fullOutputs[tool.toolId] = full.output;
    } catch (e) {
      console.warn('tool output fetch failed', e);
    }
  }
  /** What a row shows: the full output once it has been fetched, the stored
   *  preview until then. */
  function toolResponse(tool: ToolMsg): string {
    return (tool.toolId && fullOutputs[tool.toolId]) || tool.response;
  }

  // Friendly tool-use display (mirrors the NeboLoop web timeline). The backend
  // (chat_dispatch.rs humanize_tool_call) supplies `label` (gerund) + `outcome`
  // (past-tense); these helpers turn them into a doer-flavored work line.
  function fmtDuration(ms: number): string {
    return ms < 1000 ? '<1s' : `${Math.round(ms / 1000)}s`;
  }
  function workLineDuration(tools: ToolMsg[]): string {
    const total = tools.reduce((s, t) => s + (t.durationMs ?? 0), 0);
    return total > 0 ? fmtDuration(total) : '';
  }
  /** What a step did, in the past tense — the same words for a step that
   *  failed, so the summary line groups it with its siblings; the row and
   *  the summary mark the failure separately. */
  function stepOutcome(tool: ToolMsg): string {
    const resource = (tool.request as { resource?: string } | undefined)?.resource;
    return tool.outcome ?? tool.label ?? $t('chat.usedTool', { values: { name: resource || tool.name } });
  }
  function workLineLabel(tools: ToolMsg[]): string {
    const running = tools.filter((t) => t.status === 'running');
    if (running.length) {
      const cur = running[running.length - 1];
      return `${cur.label ?? $t('chat.workingWithTool', { values: { name: cur.name } })}…`;
    }
    // Group completed steps by outcome, preserving first-seen order.
    const groups = new Map<string, number>();
    for (const t of tools) groups.set(stepOutcome(t), (groups.get(stepOutcome(t)) ?? 0) + 1);
    const parts = [...groups.entries()].map(([label, n], i) => {
      let s = label;
      if (n > 1) {
        const m = label.match(/^(\w+) an? (.+)$/);
        s = m ? `${m[1]} ${n} ${m[2]}${m[2].endsWith('s') ? '' : 's'}` : `${label} ×${n}`;
      }
      return i === 0 ? s : s.charAt(0).toLowerCase() + s.slice(1);
    });
    const line = parts.slice(0, 3).join(', ');
    return groups.size > 3 ? `${line}, ${$t('chat.moreCount', { values: { count: groups.size - 3 } })}` : line;
  }

  // Tools now live on the assistant message that ran them (msg.tools[]), so there
  // are no sibling tool messages to collapse — the rendered list IS the message
  // list, one-to-one. (Kept as derived aliases so the turn-boundary + index logic
  // below reads unchanged.)
  const groupedMessages = $derived(messages);
  const originalIndices = $derived(messages.map((_, i) => i));

  // Drag-and-drop handlers
  function handleDragEnter(e: DragEvent) {
    e.preventDefault();
    if (!allowAttachments) return; // no drop affordance when files go nowhere
    dragCounter++;
    isDragging = true;
  }

  function handleDragLeave(e: DragEvent) {
    e.preventDefault();
    dragCounter--;
    if (dragCounter <= 0) {
      isDragging = false;
      dragCounter = 0;
    }
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    isDragging = false;
    dragCounter = 0;

    // If the composer already handled this drop, don't double-add
    if ((e as Event & { _composerHandled?: boolean })._composerHandled) return;

    const files = Array.from(e.dataTransfer?.files || []);
    if (files.length) {
      composerRef?.addFiles(files);
    }
  }
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

<div data-tour="chat" class="flex-1 flex min-w-0 min-h-0 overflow-hidden {resizing ? 'select-none' : ''}" bind:this={containerEl}>
<!-- Chat column -->
<div
  class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0 relative"
  role="application"
  ondragenter={handleDragEnter}
  ondragleave={handleDragLeave}
  ondragover={handleDragOver}
  ondrop={handleDrop}
>
  <!-- Dropzone overlay -->
  {#if isDragging}
    <div class="absolute inset-0 z-30 bg-primary/5 border-2 border-dashed border-primary rounded-lg flex items-center justify-center pointer-events-none">
      <div class="text-primary font-medium text-sm">{$t('chat.dropFilesHere')}</div>
    </div>
  {/if}

        {#snippet headerIcon(active: boolean, label: string, onclick: () => void, icon: Snippet)}
          <button
            class="w-7 h-7 max-md:w-10 max-md:h-10 rounded-md flex items-center justify-center cursor-pointer bg-transparent border-none transition-colors {active
              ? 'text-primary bg-primary/10'
              : 'text-base-content/60 hover:text-base-content hover:bg-base-200'}"
            {onclick}
            title={label}
            aria-label={label}
          >{@render icon()}</button>
        {/snippet}
        {#snippet computerIcon()}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="14" rx="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>
        {/snippet}
        {#snippet flowsIcon()}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="6" height="6" rx="1.5"/><rect x="15" y="15" width="6" height="6" rx="1.5"/><path d="M9 6h4a2 2 0 0 1 2 2v10"/></svg>
        {/snippet}
        {#snippet runsIcon()}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7"/><polyline points="3 3 3 8 8 8"/><polyline points="12 8 12 12 15 14"/></svg>
        {/snippet}
        {#snippet settingsIcon()}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
        {/snippet}
        {#snippet workIcon()}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/><path d="M14 9l3 3-3 3"/></svg>
        {/snippet}

  <!-- Header -->
  {#if headerTitle}
    <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
      <!-- Below md the workspace list is an off-canvas drawer, and this is its
           only opener — the top header that used to carry the hamburger is gone. -->
      <!-- Mobile is list-first: this is "back to your team", so it reads as a
           back chevron, not a hamburger. -->
      {#if onback}
      <button
        class="md:hidden w-10 h-10 -ml-2.5 rounded-md flex items-center justify-center border-none bg-transparent cursor-pointer text-base-content/70 shrink-0"
        aria-label="Employees"
        onclick={onback}
      >
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 18 9 12 15 6"/></svg>
      </button>
      {/if}
      <span class="flex items-baseline gap-2 min-w-0">
        <span class="text-sm font-semibold truncate">{agentName}</span>
        {#if isApp}
          <span class="text-[9px] uppercase tracking-wider px-1 py-px rounded bg-info/15 text-info font-semibold shrink-0">{$t('agent.appBadge')}</span>
        {/if}
        {#if isolated}
          <!-- Separate conversations only mean something when memory is sealed
               between them. MessageSquareLock = "this conversation is sealed";
               the plain Lock stays on the Settings toggle — that distinction
               is deliberate. Words live in the tooltip. -->
          <span
            class="self-center text-warning/80 shrink-0 tooltip tooltip-bottom"
            data-tip={$t('agentIsolation.isolated')}
          >
            <MessageSquareLock class="w-3 h-3" />
          </span>
        {/if}
        {#if headerTitle && headerTitle !== agentName}
          <span class="text-sm text-base-content/70 truncate">{headerTitle}</span>
        {/if}
      </span>
      <div class="ml-auto max-lg:hidden flex items-center gap-0.5 shrink-0">



        {@render headerIcon(computerFull, $t('chat.botComputer'), () => (computerFull = true), computerIcon)}
        {#if flowsPane}
          {@render headerIcon(creationsOpen && paneView === 'flows', $t('nav.flows'), () => togglePane('flows'), flowsIcon)}
        {/if}
        {#if onopenruns}
          {@render headerIcon(false, $t('nav.runs'), onopenruns, runsIcon)}
        {/if}
        {#if headerRight}
          {@render headerIcon(creationsOpen && paneView === 'work', $t('chat.work'), () => togglePane('work'), workIcon)}
        {/if}
        {#if onsettings}
          {@render headerIcon(false, $t('settings.title'), onsettings, settingsIcon)}
        {/if}
        {#if isApp && onopenapp}
          <button
            class="btn btn-primary btn-xs max-md:btn-sm gap-1 ml-1 shrink-0"
            onclick={onopenapp}
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
            {$t('agent.openApp')}
          </button>
        {/if}
      </div>

      <!-- Narrow widths: the icon row collapses into one labeled menu — five
           icons ate the title's room anywhere under lg, not just on phones. -->
      <div class="lg:hidden ml-auto shrink-0 flex items-center gap-1">
        {#if isApp && onopenapp}
          <button class="btn btn-primary btn-sm gap-1" onclick={onopenapp}>
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><polyline points="15 3 21 3 21 9"/><line x1="10" y1="14" x2="21" y2="3"/></svg>
            {$t('agent.openApp')}
          </button>
        {/if}
        <div class="dropdown dropdown-end">
          <div tabindex="0" role="button" aria-label={$t('common.more')} class="w-10 h-10 rounded-md flex items-center justify-center cursor-pointer text-base-content/70 hover:text-base-content hover:bg-base-200">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/></svg>
          </div>
          <ul class="dropdown-content menu z-[55] mt-1 w-48 rounded-box border border-base-300 bg-base-100 p-1.5 shadow-lg">
            <!-- No Computer here. Watching an employee drive a desktop is a
                 thing you do sitting down at one; on a phone it is a screen
                 you cannot use, offered where the useful actions live. It
                 stays on the icon row at wider widths. -->
            {#if flowsPane}
              <li><button onclick={() => { (document.activeElement as HTMLElement)?.blur(); togglePane('flows'); }}>{@render flowsIcon()}{$t('nav.flows')}</button></li>
            {/if}
            {#if onopenruns}
              <li><button onclick={() => { (document.activeElement as HTMLElement)?.blur(); onopenruns?.(); }}>{@render runsIcon()}{$t('nav.runs')}</button></li>
            {/if}
            {#if headerRight}
              <li><button onclick={() => { (document.activeElement as HTMLElement)?.blur(); togglePane('work'); }}>{@render workIcon()}{$t('chat.work')}</button></li>
            {/if}
            {#if onsettings}
              <li><button onclick={() => { (document.activeElement as HTMLElement)?.blur(); onsettings?.(); }}>{@render settingsIcon()}{$t('settings.title')}</button></li>
            {/if}
          </ul>
        </div>
      </div>
    </div>
  {/if}

  <!-- Messages / Empty state -->
  {#if !hasMessages && historyLoading}
    <div class="flex-1 flex items-center justify-center p-6">
      <span class="loading loading-spinner loading-md text-base-content/70"></span>
    </div>
  {:else if !hasMessages && emptyTitle}
    <div class="flex-1 flex flex-col items-center justify-center gap-4 p-6">
      {#if emptyIcon}
        <div class="w-12 h-12 rounded-box flex items-center justify-center font-mono text-xl font-semibold bg-primary text-primary-content">{emptyIcon}</div>
        <div class="text-base font-semibold">{emptyTitle}</div>
      {:else}
        <div class="text-2xl font-semibold text-base-content">{emptyTitle}</div>
      {/if}
      {#if emptyDesc}
        <div class="text-sm text-base-content/70 text-center max-w-[320px] leading-relaxed">{emptyDesc}</div>
      {/if}
    </div>
  {:else}
  <div class="flex-1 relative min-h-0">
    <!-- Scroll to bottom button -->
    {#if showScrollButton}
      <div class="absolute bottom-4 left-1/2 -translate-x-1/2 z-10">
        <button
          type="button"
          onclick={scrollToBottom}
          class="p-2 rounded-full bg-base-200 border border-base-300 text-base-content/90 hover:bg-base-300 hover:text-base-content transition-all shadow-lg cursor-pointer"
          title={$t('chat.scrollToBottom')}
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><polyline points="19 12 12 19 5 12"/></svg>
        </button>
      </div>
    {/if}
  <div bind:this={messagesContainer} role="log" onscroll={handleScroll} onwheel={handleWheel} ontouchstart={handleTouchStart} ontouchend={handleTouchEnd} class="chat-scroller h-full overflow-y-auto p-[18px_24px]">
  <div bind:this={messagesContent} class="max-w-3xl mx-auto flex flex-col gap-1" data-selectable>
    {#if isLoadingMore}
      <div class="flex justify-center py-3">
        <div class="loading loading-spinner loading-sm text-base-content/30"></div>
      </div>
    {/if}

    <!-- The activity panel for one turn: notes and tool calls in order,
         folded under a summary line. Rows open on click; a page's address
         is a link. keyId = the turn's first segment id. -->
    {#snippet activityPanel(steps: ActivityStep[], tools: ToolMsg[], keyId: string, live: boolean)}
      {@const open = activityOpen[keyId] ?? false}
      <div class="max-w-[640px] my-1.5">
        <button
          type="button"
          class="flex items-center gap-1.5 text-xs text-base-content/70 cursor-pointer bg-transparent border-none p-0 hover:text-base-content/90 transition-colors"
          aria-expanded={open}
          onclick={() => (activityOpen[keyId] = !open)}
        >
          <span class="truncate max-w-[60vw] md:max-w-md {live ? 'activity-live' : ''}">{tools.length ? workLineLabel(tools) : $t('chat.working')}</span>
          <span class="shrink-0 transition-transform {open ? 'rotate-90' : ''}">&rsaquo;</span>
        </button>

        {#if open}
          <div class="mt-1.5 rounded-xl border border-base-300 bg-base-100 divide-y divide-base-300 overflow-hidden">
            {#each steps as step (step.key)}
              {#if step.kind === 'note'}
                {@const expandable = step.lines.length > 1}
                {@const isExpanded = !!expandedResults[step.key]}
                <div class="px-3 py-2 text-xs">
                  <button
                    type="button"
                    class="flex w-full items-center gap-1.5 text-left bg-transparent border-none p-0 text-base-content/70 {expandable ? 'cursor-pointer hover:text-base-content/90' : 'cursor-default'}"
                    disabled={!expandable}
                    aria-expanded={expandable ? isExpanded : undefined}
                    onclick={() => toggleResult(step.key)}
                  >
                    <span class="truncate flex-1">{plainNote(step.lines[step.lines.length - 1])}</span>
                    {#if expandable}<span class="shrink-0 transition-transform {isExpanded ? 'rotate-90' : ''}">&rsaquo;</span>{/if}
                  </button>
                  {#if expandable && isExpanded}
                    <div class="mt-1.5 flex flex-col gap-1 text-base-content/70">
                      {#each step.lines as line}<p class="m-0">{plainNote(line)}</p>{/each}
                    </div>
                  {/if}
                </div>
              {:else}
                {@const tool = step.tool}
                {@const meta = stepMeta(tool.request)}
                {@const expandable = canExpand(tool)}
                {@const isExpanded = !!expandedResults[step.key]}
                {@const cmd = shellCommand(tool)}
                <div class="px-3 py-2 text-xs">
                  <div class="flex items-center gap-2 min-w-0">
                    {#if tool.status === 'running'}
                      <svg width="12" height="12" viewBox="0 0 18 18" class="text-primary shrink-0 animate-spin"><circle cx="9" cy="9" r="6" stroke="currentColor" stroke-width="1.5" fill="none" stroke-dasharray="22 16" stroke-linecap="round"/></svg>
                    {/if}
                    <button
                      type="button"
                      class="flex min-w-0 items-center gap-2 text-left bg-transparent border-none p-0 {expandable ? 'cursor-pointer' : 'cursor-default'} {meta?.href ? 'shrink-0' : 'flex-1'}"
                      disabled={!expandable}
                      aria-expanded={expandable ? isExpanded : undefined}
                      onclick={() => toggleResult(step.key, tool)}
                    >
                      <span class="shrink-0 text-base-content/70">{tool.status === 'running' ? (tool.label ?? tool.name) : stepOutcome(tool)}{#if tool.status === 'running' && tool.statusText}<span class="text-base-content/70 ml-1">{tool.statusText}</span>{/if}</span>
                      {#if tool.status === 'error'}<span class="shrink-0 text-error">{$t('chat.failed')}</span>{/if}
                      {#if meta && !meta.href}<span class="truncate text-base-content/80" title={meta.text}>{meta.text}</span>{/if}
                      {#if $devMode}<span class="font-mono text-base-content/70 shrink-0">{tool.name}</span>{/if}
                      {#if tool.durationMs}<span class="text-base-content/70 shrink-0">{fmtDuration(tool.durationMs)}</span>{/if}
                      {#if expandable && !meta?.href}<span class="shrink-0 text-base-content/70 transition-transform {isExpanded ? 'rotate-90' : ''}">&rsaquo;</span>{/if}
                    </button>
                    {#if meta?.href}
                      <a href={meta.href} target="_blank" rel="noopener noreferrer" class="truncate text-primary underline flex-1" title={meta.href}>{meta.text}</a>
                      {#if expandable}
                        <button type="button" class="shrink-0 bg-transparent border-none p-0 cursor-pointer text-base-content/70 transition-transform {isExpanded ? 'rotate-90' : ''}" aria-expanded={isExpanded} aria-label={$t('chat.result')} onclick={() => toggleResult(step.key, tool)}>&rsaquo;</button>
                      {/if}
                    {/if}
                  </div>
                  {#if isExpanded}
                    <div class="mt-2 flex flex-col gap-2">
                  {#if researchState(tool)}
                    {@const rs = researchState(tool)!}
                    <div class="mt-2 max-w-[560px] rounded-xl border border-base-300 bg-base-100 px-3.5 py-3">
                      <div class="flex items-center gap-2">
                        <span class="text-sm font-medium truncate flex-1">{rs.question ?? $t('chat.researchTitle')}</span>
                        {#if rs.depth}<span class="text-xs text-base-content/70 font-mono shrink-0">{rs.depth}</span>{/if}
                      </div>
                      <div class="flex items-center gap-1.5 mt-1 text-xs text-base-content/70">
                        {#if !rs.complete}
                          <span class="loading loading-spinner loading-xs text-primary"></span>
                          <span>{$t('chat.researchCounting', { values: { n: rs.sources_read ?? 0 } })}</span>
                          {#if rs.started_ms}<span class="text-base-content/70 font-mono">· {fmtElapsed(clockNow - rs.started_ms)}</span>{/if}
                          {#if researchPhaseLabel(rs.phase)}<span class="text-base-content/70">· {researchPhaseLabel(rs.phase)}</span>{/if}
                        {:else}
                          <span class="w-1.5 h-1.5 rounded-full bg-success"></span>
                          <span>{$t('chat.researchComplete', { values: { n: rs.sources_read ?? 0 } })}</span>
                          {#if rs.elapsed_ms}<span class="text-base-content/70 font-mono">· {fmtElapsed(rs.elapsed_ms)}</span>{/if}
                        {/if}
                      </div>
                      {#if rs.angles?.length}
                        <div class="mt-2 flex flex-wrap gap-1.5">
                          {#each rs.angles as angle}
                            <span class="text-xs bg-base-200 rounded-full px-2 py-0.5 text-base-content/70">{angle}</span>
                          {/each}
                        </div>
                      {/if}
                      {#if topDomains(rs.domains).length}
                        {@const top = topDomains(rs.domains)}
                        {@const max = top[0][1]}
                        <div class="mt-2.5 flex flex-col gap-1.5">
                          {#each top as [host, count]}
                            <div class="flex items-center gap-2 text-xs">
                              <img src="https://www.google.com/s2/favicons?domain={host}&sz=32" alt="" loading="lazy" class="w-3.5 h-3.5 rounded-sm shrink-0" onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')} />
                              <span class="truncate w-36">{host}</span>
                              <span class="text-base-content/70 font-mono shrink-0">{count}</span>
                              <div class="flex-1 h-1.5 rounded-full bg-base-200 overflow-hidden"><div class="h-full bg-base-content/20" style:width="{Math.round((count / max) * 100)}%"></div></div>
                            </div>
                          {/each}
                          {#if otherDomainCount(rs.domains) > 0}
                            <span class="text-xs text-base-content/70">{$t('chat.researchOtherDomains', { values: { n: otherDomainCount(rs.domains) } })}</span>
                          {/if}
                        </div>
                      {/if}
                      {#if rs.claims_verified}
                        <div class="mt-2 text-xs text-base-content/50">{$t('chat.researchVerified', { values: { n: rs.claims_verified } })}</div>
                      {/if}
                    </div>
                  {/if}
                  {#if runReceipt(tool)}
                    {@const rr = runReceipt(tool)!}
                    <div class="mt-2 max-w-[560px] rounded-xl border border-base-300 bg-base-100 px-3.5 py-3">
                      <div class="flex items-center gap-2">
                        <span class="text-sm font-medium truncate flex-1">{rr.workflow ?? $t('chat.runReceiptTitle')}</span>
                        <span class="py-0 px-1.5 rounded text-xs font-medium shrink-0 {rr.status === 'completed' ? 'bg-success/10 text-success' : rr.status === 'failed' ? 'bg-error/10 text-error' : 'bg-warning/10 text-warning'}">
                          {rr.status === 'completed' ? $t('common.completed') : rr.status === 'failed' ? $t('common.failed') : rr.status}
                        </span>
                      </div>
                      {#if rr.display?.input?.line}
                        <div class="text-xs text-base-content/50 truncate mt-0.5">{rr.display.input.line}</div>
                      {/if}
                      <div class="mt-2 flex flex-col gap-1">
                        {#each receiptLines(rr) as l}
                          <div class="flex items-start gap-2 text-xs">
                            <span class="shrink-0 {l.failed ? 'text-error' : l.stopped ? 'text-info' : 'text-success'}">{l.failed ? '✗' : l.stopped ? '⏹' : '✓'}</span>
                            <span class="text-base-content/70 min-w-0">{l.line}</span>
                          </div>
                        {/each}
                      </div>
                      {#if rr.error}
                        <div class="mt-1.5 text-xs text-error">{rr.error}</div>
                      {/if}
                      <a href="?run={rr.runId}" class="inline-block mt-2 text-xs text-primary no-underline hover:underline">{$t('chat.openRun')}</a>
                    </div>
                  {/if}
                  {#if searchPayload(tool)}
                    {#each searchPayload(tool)!.groups as g}
                      <div class="mt-2 max-w-[560px]">
                        <div class="flex items-baseline gap-2 text-xs">
                          <span class="text-base-content/70 truncate">{g.query}</span>
                          <span class="ml-auto text-base-content/50 font-mono shrink-0">{g.results.length} {g.results.length === 1 ? 'result' : 'results'}</span>
                        </div>
                        {#if g.results.length}
                          <div class="mt-1.5 rounded-xl border border-base-300 bg-base-100 divide-y divide-base-content/5 overflow-hidden">
                            {#each g.results.slice(0, 6) as r}
                              <a href={r.url} target="_blank" rel="noopener noreferrer" class="flex items-center gap-2.5 px-3 py-2 hover:bg-base-200/50 transition-colors no-underline text-inherit">
                                <img src="https://www.google.com/s2/favicons?domain={resultHost(r.url)}&sz=32" alt="" loading="lazy" class="w-4 h-4 rounded-sm shrink-0" onerror={(e) => ((e.currentTarget as HTMLImageElement).style.visibility = 'hidden')} />
                                <span class="text-xs font-medium truncate flex-1">{r.title}</span>
                                <span class="text-xs text-base-content/50 truncate max-w-[160px] shrink-0">{resultHost(r.url)}</span>
                              </a>
                            {/each}
                          </div>
                        {/if}
                      </div>
                    {/each}
                  {/if}
                      {#if cmd}
                        <div class="rounded-lg bg-base-200/60 px-3 py-2 max-h-[200px] overflow-y-auto">
                          <div class="text-[11px] font-mono text-base-content/50 mb-1">bash</div>
                          <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap m-0">{cmd}</pre>
                        </div>
                        {#if tool.response}
                          <div class="rounded-lg bg-base-200/60 px-3 py-2 max-h-[200px] overflow-y-auto">
                            <div class="text-[11px] font-medium text-base-content/50 mb-1">{$t('chat.output')}</div>
                            <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap m-0">{toolResponse(tool)}</pre>
                          </div>
                        {/if}
                      {:else if !searchPayload(tool) && !researchState(tool) && !runReceipt(tool)}
                        <div class="rounded-lg border border-base-300 bg-base-100 overflow-y-auto max-h-[200px]">
                          <div class="px-3 pt-2 pb-1.5">
                            <div class="text-[11px] font-medium text-base-content/50 mb-1">{$t('chat.request')}</div>
                            <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap m-0">{JSON.stringify(tool.request, null, 2)}</pre>
                          </div>
                          {#if tool.response}
                            <div class="px-3 pt-1.5 pb-2 border-t border-base-300">
                              <div class="text-[11px] font-medium text-base-content/50 mb-1">{$t('chat.output')}</div>
                              <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap m-0">{toolResponse(tool)}</pre>
                            </div>
                          {/if}
                        </div>
                      {/if}
                    </div>
                  {/if}
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      </div>
    {/snippet}

    <!-- Keyed by id: loading older history prepends rows, and an unkeyed list
         would re-render every existing row in place (a blank pane until the
         next scroll on WebKit). Transient rows without an id key by position. -->
    {#each groupedMessages as msg, idx ('id' in msg && msg.id ? msg.id : idx)}
      {#if msg.type === 'user'}
        {@const origIdx = originalIndices[idx]}
        {#if editingIdx === origIdx}
          <!-- Inline edit box -->
          <div class="w-full mt-3">
            <div class="rounded-box border border-base-300 shadow-md p-3 bg-surface">
              <textarea
                bind:this={editTextareaEl}
                bind:value={editText}
                rows="1"
                class="w-full text-sm outline-none resize-none bg-transparent leading-relaxed min-h-[2.5rem]"
                onkeydown={(e) => handleEditKeydown(e, origIdx)}
                oninput={handleEditInput}
              ></textarea>
              <div class="flex items-center justify-between mt-2 pt-2 border-t border-base-content/10">
                <span class="text-xs text-base-content/50">{$t('chat.enterToSubmit')}</span>
                <div class="flex items-center gap-2">
                  <button
                    class="py-1.5 px-3 rounded-lg text-xs cursor-pointer border border-base-300 bg-transparent hover:bg-base-200 transition-colors"
                    onclick={cancelEdit}
                  >{$t('common.cancel')}</button>
                  <button
                    class="py-1.5 px-3 rounded-lg text-xs font-medium cursor-pointer border-none bg-primary text-primary-content hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                    disabled={!editText.trim()}
                    onclick={() => saveEdit(origIdx)}
                  >{$t('chat.saveAndSubmit')}</button>
                </div>
              </div>
            </div>
          </div>
        {:else}
          <!-- A post that reached this employee through a team reads the way
               the team's own thread reads it: who said it and in which team,
               then the words — never the envelope the model was handed. A
               teammate's post sits on the left like a reply; the owner's on
               the right like their other messages. -->
          {@const tp = msg.teamPost}
          {@const fromOwner = !tp || (tp.fromOwner ?? tp.from === 'Owner')}
          <div class="max-w-[640px] mt-3 {fromOwner ? 'self-end' : ''}" data-user-msg>
            {#if tp}
              <div class="text-xs font-medium text-base-content/60 mb-1 {fromOwner ? 'text-right' : ''}">{fromOwner ? $t('common.you') : tp.from} · {tp.teamName}</div>
            {/if}
            <div class="py-2.5 px-3.5 rounded-xl text-sm leading-relaxed bg-base-200 {fromOwner ? 'rounded-br-sm' : 'rounded-bl-sm'} prose prose-sm max-w-none {msg.pending ? 'italic text-base-content/60' : ''} [&_p]:my-0 [&_ul]:my-1 [&_ol]:my-1 [&>:first-child]:mt-0 [&>:last-child]:mb-0">
              {#if tp}
                {@html renderMarkdown(tp.text)}
              {:else if parseLargeInput(msg.content)}
                {@const li = parseLargeInput(msg.content)!}
                <div class="not-prose mb-2 text-xs text-base-content/50">
                  {$t('chat.largeInputNote', { values: { chars: li.chars } })}
                </div>
                {@html renderMarkdown(li.summary)}
              {:else}
                {@html renderMarkdown(stripAttachmentNotes(msg.content))}
              {/if}
              {#if msg.attachments?.length}
                <div class="flex flex-wrap gap-2 mt-2">
                  {#each msg.attachments as att}
                    {@const attType = getAttachmentType(att.mimeType)}
                    {#if attType === 'image'}
                      <button type="button" class="block p-0 bg-transparent border-0 cursor-zoom-in" onclick={() => (lightboxUrl = attSrc(att))} aria-label={$t('chat.viewImage')}>
                        <img
                          src={attSrc(att)}
                          alt={att.filename}
                          class="max-w-[240px] max-h-[180px] rounded-lg border border-base-content/15 object-cover"
                          loading="lazy"
                        />
                      </button>
                    {:else if attType === 'video'}
                      <video
                        src={attSrc(att)}
                        controls
                        preload="metadata"
                        class="max-w-[320px] max-h-[240px] rounded-lg border border-base-content/15"
                      >
                        <track kind="captions" />
                      </video>
                    {:else if attType === 'audio'}
                      <audio src={attSrc(att)} controls preload="metadata" class="max-w-[280px]"></audio>
                    {:else}
                      <a
                        href={attSrc(att)}
                        download={att.filename}
                        class="flex items-center gap-2 py-2 px-3 rounded-lg border border-base-content/15 bg-base-200/50 hover:bg-base-200 transition-colors no-underline text-inherit"
                      >
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
                        <span class="text-xs font-medium truncate max-w-[160px]">{att.filename}</span>
                        <span class="text-xs text-base-content/50 font-mono shrink-0">{formatFileSize(att.size)}</span>
                      </a>
                    {/if}
                  {/each}
                </div>
              {/if}
            </div>
            <div class="flex items-center gap-1 mt-1.5 {fromOwner ? 'justify-end' : ''}">
              {#if msg.pending}
                <span class="text-xs text-base-content/50 italic mr-1">{$t('chat.pending')}</span>
              {:else if msg.time}
                <span class="text-xs text-base-content/50 font-mono mr-1">{msg.time}</span>
              {/if}
              {#if !tp}
                <button
                  class="w-7 h-7 rounded-md grid place-items-center text-base-content/50 hover:text-base-content hover:bg-base-200 cursor-pointer bg-transparent border-none transition-colors"
                  title={$t('chat.editResend')}
                  onclick={() => startEdit(origIdx, msg.content)}
                >
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
                </button>
              {/if}
              <button
                class="w-7 h-7 rounded-md grid place-items-center {copiedIdx === origIdx ? 'text-success' : 'text-base-content/50 hover:text-base-content hover:bg-base-200'} cursor-pointer bg-transparent border-none transition-colors"
                title={copiedIdx === origIdx ? $t('chat.copied') : $t('common.copy')}
                onclick={() => copyMessage(tp ? tp.text : msg.content, origIdx)}
              >
                {#if copiedIdx === origIdx}
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                {:else}
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                {/if}
              </button>
            </div>
          </div>
        {/if}

      {:else if msg.type === 'thinking'}
        <details class="max-w-[640px] mt-2 mb-1">
          <summary class="text-xs text-base-content/50 cursor-pointer hover:text-base-content/70 transition-colors">
            {$t('chat.workedFor', { values: { duration: msg.duration } })}
          </summary>
          <div class="mt-1.5 py-2 px-3 rounded-box bg-base-200 border-l-2 border-base-content/20 text-xs leading-relaxed font-mono whitespace-pre-wrap">{msg.content}</div>
        </details>

      {:else if msg.type === 'ask'}
        <div class="max-w-[640px] mt-3">
          <AskWidget
            requestId={msg.requestId}
            prompt={msg.prompt}
            widgets={msg.widgets}
            response={msg.response}
            cancelled={msg.cancelled ?? false}
            disabled={!isLoading}
            onSubmit={(id, val) => onasksubmit?.(id, val)}
          />
          {#if !msg.response && askQueueLength > 0}
            <p class="mt-1.5 text-xs text-base-content/50">{$t('chat.moreQuestionsWaiting', { values: { count: askQueueLength } })}</p>
          {/if}
        </div>

      {:else if msg.type === 'assistant'}
        {@const isTurnStart = idx === 0 || groupedMessages[idx - 1]?.type !== 'assistant'}
        <!-- One assistant TURN is one container, rendered from its first
             segment: the activity panel (notes + tools, folded), then the
             answer, then attachments, artifact cards, and the time/copy/retry
             row. Later segments of the same turn render nothing themselves. -->
        {#if isTurnStart}
          {@const origIdx = originalIndices[idx]}
          {@const segs = turnSegments(idx)}
          {@const last = segs[segs.length - 1]}
          {@const lastIdx = idx + segs.length - 1}
          {@const lastOrigIdx = originalIndices[lastIdx]}
          {@const nextGroup = groupedMessages[lastIdx + 1]}
          {@const isTurnEnd = nextGroup ? (nextGroup.type === 'user' || nextGroup.type === 'ask') : !isLoading}
          {@const keyId = msg.id ?? `m${origIdx}`}
          {@const turnTools = segs.flatMap((sg) => shownTools(sg.tools))}
          {@const steps = activitySteps(segs, keyId)}
          {@const answer = turnAnswer(segs)}
          {@const turnAttachments = segs.flatMap((sg) => sg.attachments ?? [])}
          <div class="max-w-[640px] mt-3">
            {#if msg.delegateAgentName}
              {@const da = allAgents.find(a => a.id === msg.delegateAgentId)}
              <div class="flex items-center gap-1.5 mb-1">
                <AgentAvatar name={da?.name ?? msg.delegateAgentName} color={da?.color} size="xs" />
                <span class="text-xs font-medium">{msg.delegateAgentName}</span>
              </div>
            {/if}
            {#if steps.length}
              {@render activityPanel(steps, turnTools, keyId, !isTurnEnd)}
            {/if}
            {#if answer}
              <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
              <div class="text-sm leading-relaxed prose prose-sm max-w-none" onclick={handleWorkMentionClick}>
                {@html linkWorkMentions(renderMarkdown(answer), (last as any).workItems)}
              </div>
            {/if}
            {#each segs.flatMap((sg) => coworkerEvents(sg.tools)) as ev, evIdx (evIdx)}
              <a
                href={ev.threadKey ? cwHref(ev.threadKey) : undefined}
                class="flex items-center justify-center gap-1.5 my-2.5 text-xs text-base-content/60 no-underline {ev.threadKey ? 'hover:text-base-content transition-colors' : ''}"
                title={ev.threadKey ? $t('coworkerThread.open') : undefined}
              >
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0"><path d="M22 2 11 13"/><path d="M22 2 15 22l-4-9-9-4Z"/></svg>
                <span>{$t('chat.messagedCoworker')}</span>
                <span class="font-medium text-base-content/80">{ev.to}</span>
              </a>
            {/each}
            {#each segs.flatMap((sg) => consentLines(sg.tools)) as consent, cIdx (cIdx)}
              <ConsentChip {consent} />
            {/each}
          {#if turnAttachments.length}
            <div class="flex flex-wrap gap-2 mt-2">
              {#each turnAttachments as att}
                {@const attType = getAttachmentType(att.mimeType)}
                {#if attType === 'image'}
                  <button type="button" class="block p-0 bg-transparent border-0 cursor-zoom-in" onclick={() => (lightboxUrl = attSrc(att))} aria-label={$t('chat.viewImage')}>
                    <img
                      src={attSrc(att)}
                      alt={att.filename}
                      class="max-w-[240px] max-h-[180px] rounded-lg border border-base-content/15 object-cover"
                      loading="lazy"
                    />
                  </button>
                {:else if attType === 'video'}
                  <video
                    src={attSrc(att)}
                    controls
                    preload="metadata"
                    class="max-w-[320px] max-h-[240px] rounded-lg border border-base-content/15"
                  >
                    <track kind="captions" />
                  </video>
                {:else if attType === 'audio'}
                  <audio src={attSrc(att)} controls preload="metadata" class="max-w-[280px]"></audio>
                {:else}
                  <a
                    href={attSrc(att)}
                    download={att.filename}
                    class="flex items-center gap-2 py-2 px-3 rounded-lg border border-base-content/15 bg-base-200/50 hover:bg-base-200 transition-colors no-underline text-inherit"
                  >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
                    <span class="text-xs font-medium truncate max-w-[160px]">{att.filename}</span>
                    <span class="text-xs text-base-content/50 font-mono shrink-0">{formatFileSize(att.size)}</span>
                  </a>
                {/if}
              {/each}
            </div>
          {/if}
          <!-- Inline artifact cards for this message (populated by agent tool results) -->
          {#each artifacts.filter(a => segs.some((sg) => sg.id === a.messageId)) as artifact}
            {@const ArtIcon = artifactIcons[artifact.kind]}
            <button
              class="flex items-center gap-3 mt-3 w-full max-w-xs p-3 rounded-xl border cursor-pointer transition-colors text-left {activeArtifactId === artifact.id && creationsOpen ? 'border-primary/40 bg-primary/5' : 'border-base-content/10 bg-base-200/30 hover:border-base-content/20 hover:bg-base-200/50'}"
              onclick={() => openArtifact(artifact.id)}
            >
              {#if ArtIcon}<ArtIcon class="w-4 h-4 text-base-content/50 shrink-0" />{/if}
              <div class="flex-1 min-w-0">
                <div class="text-xs font-medium truncate">{artifact.title}</div>
                <div class="text-xs text-base-content/50">{artifact.kind === 'code' ? $t('chat.artifactCode') : artifact.kind === 'table' ? $t('chat.artifactSpreadsheet') : artifact.kind === 'slides' ? $t('chat.artifactPresentation') : $t('chat.artifactDocument')}</div>
              </div>
            </button>
          {/each}

          {#if isTurnEnd}
            <div class="flex items-center gap-1 mt-2">
              {#if last.time}
                <span class="text-xs text-base-content/50 font-mono mr-1">{last.time}</span>
              {/if}
              <button
                class="w-7 h-7 rounded-md grid place-items-center {copiedIdx === lastOrigIdx ? 'text-success' : 'text-base-content/50 hover:text-base-content hover:bg-base-200'} cursor-pointer bg-transparent border-none transition-colors"
                title={copiedIdx === lastOrigIdx ? $t('chat.copied') : $t('common.copy')}
                onclick={() => copyMessage(answer, lastOrigIdx)}
              >
                {#if copiedIdx === lastOrigIdx}
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>
                {:else}
                  <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg>
                {/if}
              </button>
              <button
                class="w-7 h-7 rounded-md grid place-items-center text-base-content/50 hover:text-base-content hover:bg-base-200 cursor-pointer bg-transparent border-none transition-colors"
                title={$t('common.retry')}
                onclick={() => redoMessage(origIdx)}
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
              </button>
            </div>
          {/if}
          </div>
        {/if}
      {/if}
    {/each}

    <!-- Live working indicator (ChatGPT/Claude style): shown for the WHOLE run,
         including while the reply text is streaming or a tool grinds after the
         last text chunk — not only before the first assistant message. -->
    {#if isLoading && groupedMessages.length > 0}
      <div class="max-w-[640px] mt-3 py-2 flex items-center gap-2">
        <span class="loading loading-spinner loading-xs text-primary"></span>
        <span class="text-sm text-base-content/70 animate-pulse">{activityStatus || $t('chat.working')}</span>
      </div>
    {/if}
    {#if !isLoading && helpers.length > 0}
      <div class="helper-status">
        <span class="loading loading-dots loading-xs"></span>
        <span>{helpers.length === 1
          ? $t('chat.helperWorking', { values: { what: helpers[0].activity || helpers[0].description } })
          : $t('chat.helpersWorking', { values: { n: helpers.length } })}</span>
      </div>
    {/if}
    <!-- The owner recap (WP2.5): one or two plain sentences under the
         finished turn, for coming back to this thread. -->
    {#if !isLoading && recapText}
      <div class="max-w-[640px] mt-2 text-xs text-base-content/60 italic">
        {recapText}
      </div>
    {/if}
  </div>
  <!-- Reserved room for the streaming reply — see turnSpacerHeight. Outside the
       observed content wrapper so spacer changes don't re-fire the observer. -->
  {#if turnSpacerHeight > 0}
    <div style="height: {turnSpacerHeight}px" aria-hidden="true"></div>
  {/if}
  </div>
  </div>
  {/if}

  <!-- Agreed goal: what the work continues toward until a check confirms it -->
  {#if goal}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-4 mb-2">
      <div class="px-3 py-2 rounded-lg bg-base-200 text-xs text-base-content/80">
        <span class="font-medium">{$t('chat.goalLine', { values: { condition: goal.condition, turns: goal.turns } })}</span>
        {#if goal.last_reason}
          <span> · {$t('chat.goalLastCheck', { values: { reason: goal.last_reason } })}</span>
        {/if}
        {#if goal.status.startsWith('paused')}
          <span class="text-warning"> · {$t('chat.goalPaused')}</span>
        {/if}
      </div>
    </div>
  {/if}

  <!-- Quota warning banner -->
  {#if quotaWarning}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-4 mb-2">
      <div class="px-3 py-2 rounded-lg bg-warning/10 border border-warning/30 flex items-center justify-between">
        <span class="text-xs text-warning-content">{quotaWarning}</span>
        <button class="btn btn-ghost btn-xs" onclick={() => ondismisswarning?.()}>x</button>
      </div>
    </div>
  {/if}

  <!-- Chat error banner (run died before producing a reply) -->
  {#if chatError}
    {@const isOutOfBalance = chatError.includes('USAGE_LIMIT_EXCEEDED')}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-4 mb-2">
      <div class="px-3 py-2 rounded-lg bg-error/10 border border-error/30 flex items-center justify-between gap-3">
        <span class="text-xs text-base-content">
          {#if isOutOfBalance}
            {$t('chat.outOfBalance')}
          {:else}
            {chatError}
          {/if}
        </span>
        <div class="flex items-center gap-2 shrink-0">
          {#if isOutOfBalance}
            <button type="button" class="btn btn-primary btn-xs" onclick={openWebBilling}>{$t('chat.topUp')}</button>
          {/if}
          <button class="btn btn-ghost btn-xs" onclick={() => ondismisserror?.()}>x</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Teach-a-task record bar -->
  {#if teachActive || teachError}
    <div class="max-w-3xl mx-auto w-full shrink-0 mb-2">
      <div class="flex items-center gap-2.5 rounded-lg px-3 py-2 {teachError ? 'bg-error/10 text-error' : 'bg-error/10'}">
        {#if teachActive}
          <span class="w-2 h-2 rounded-full bg-error animate-pulse"></span>
          <span class="text-sm">{$t('chat.watchingAndLearning', { values: { name: agentName } })}</span>
          <span class="text-xs tabular-nums text-base-content/60">{fmtTeach(teachSeconds)}</span>
          <button type="button" class="btn btn-error btn-xs ml-auto normal-case" onclick={stopTeach}>
            {$t('chat.stopRecording')}
          </button>
        {:else}
          <span class="text-sm">{teachError}</span>
          <button type="button" class="btn btn-ghost btn-xs ml-auto" onclick={() => (teachError = '')}>✕</button>
        {/if}
      </div>
    </div>
  {/if}

  <!-- The asks this chat's work is waiting on: the same card as the Inbox;
       answered anywhere, it leaves everywhere. -->
  {#if chatAsks.length > 0}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-4 mb-2 flex flex-col gap-2">
      {#each chatAsks as ask (ask.id)}
        <PermissionAskCard {ask} via="chat" />
      {/each}
    </div>
  {/if}

  <!-- Composer — sits on the home-indicator edge on phones. -->
  <div class="max-w-3xl mx-auto w-full shrink-0 max-md:pb-[env(safe-area-inset-bottom)]">
    <ChatComposer
      {agentName}
      {agentId}
      {threadId}
      {sessionId}
      {placeholder}
      {allAgents}
      onsend={handleSend}
      {onstop}
      {isLoading}
      {allowAttachments}
      onteach={startTeach}
      prefill={composerPrefill}
      {onprefilled}
      bind:this={composerRef}
    />
  </div>
</div>

<!-- Resize handle + Creations panel -->
{#if computerFull}
  <!-- The computer takes the window. A remote desktop scaled into a side rail
       is unusable, and this is the one surface you drive rather than refer to. -->
  <div class="fixed inset-0 z-[80] bg-neutral flex flex-col">
    <DesktopView
      onclose={() => (computerFull = false)}
      onrecord={() => (teachActive ? stopTeach() : startTeach())}
      recording={teachActive}
    />
  </div>
{/if}

{#if creationsOpen}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="max-md:hidden w-1.5 shrink-0 cursor-col-resize relative z-10 group bg-base-200 hover:bg-primary/30 transition-colors {resizing ? '!bg-primary/50' : ''}"
    onmousedown={startResize}
    role="separator"
    aria-orientation="vertical"
    tabindex="0"
  >
    <!-- Wider invisible hit area so the drag is easy to grab -->
    <div class="absolute inset-y-0 -left-2 -right-2"></div>
    <!-- Grip handle — always faintly visible, solid on hover/drag -->
    <div class="absolute top-1/2 -translate-y-1/2 left-1/2 -translate-x-1/2 w-3 h-10 rounded-full bg-base-300 border border-base-content/10 flex items-center justify-center opacity-60 group-hover:opacity-100 transition-opacity {resizing ? '!opacity-100' : ''}">
      <div class="flex flex-col gap-0.5">
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/40"></div>
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/40"></div>
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/40"></div>
      </div>
    </div>
  </div>
  <!-- Creations panel. pointer-events-none while dragging the divider: the
       viewer iframe otherwise swallows mousemove and the resize stalls. -->
  <div class="flex flex-col bg-base-100 min-h-0 min-w-0 overflow-hidden shrink-0 border-l border-base-300 max-md:fixed max-md:inset-0 max-md:z-[60] max-md:!w-full max-md:border-l-0 {workFull ? 'fixed inset-0 z-[65] !w-full border-l-0' : ''} {resizing ? 'pointer-events-none' : ''}" style="width: {creationsWidth}px">
    <!-- Creations header -->
    <div class="h-11 px-4 border-b border-base-content/10 flex items-center gap-2 shrink-0">
      {#if activeArtifact}
        {@const ActiveIcon = artifactIcons[activeArtifact.kind]}
        <!-- Active file + dropdown list of every artifact in the thread
             (a tab strip stops scaling past a handful of files). -->
        <div class="dropdown flex-1 min-w-0">
          <div tabindex="0" role="button" class="flex items-center gap-1.5 py-1 px-2 rounded-md text-xs font-medium cursor-pointer hover:bg-base-200 transition-colors max-w-full w-fit">
            {#if ActiveIcon}<ActiveIcon class="w-3 h-3 shrink-0" />{/if}
            <span class="truncate">{activeArtifact.title}</span>
            {#if activeVersionList.length > 1}
              <span class="text-xs text-base-content/50 font-mono shrink-0">v{activeArtifact.version}</span>
            {/if}
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0 text-base-content/50"><polyline points="6 9 12 15 18 9"/></svg>
          </div>
          <ul class="dropdown-content menu menu-sm bg-base-100 border border-base-300 rounded-box z-50 w-72 max-h-80 overflow-y-auto flex-nowrap p-1 shadow-md">
            {#if documents.length > 1}
              <li class="menu-title"><span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('chat.documents')}</span></li>
            {/if}
            {#each documents as d}
              {@const ArtIcon2 = artifactIcons[d.kind]}
              <li>
                <button
                  class="flex items-center gap-2 {activeArtifactId === d.documentId ? 'bg-base-200 font-medium' : ''}"
                  onclick={() => { openArtifact(d.documentId); (document.activeElement as HTMLElement | null)?.blur(); }}
                >
                  {#if ArtIcon2}<ArtIcon2 class="w-3.5 h-3.5 shrink-0 text-base-content/70" />{/if}
                  <span class="truncate text-xs">{d.title}</span>
                </button>
              </li>
            {/each}
            {#if activeVersionList.length > 1}
              <li class="menu-title"><span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('chat.versions')}</span></li>
              <li>
                <button
                  class="flex items-center justify-between gap-2 {activeVersion == null ? 'bg-base-200 font-medium' : ''}"
                  onclick={() => { activeVersion = null; (document.activeElement as HTMLElement | null)?.blur(); }}
                >
                  <span class="text-xs">{$t('chat.latest')}</span>
                  <span class="text-xs text-base-content/50 font-mono">v{activeVersionList.length}</span>
                </button>
              </li>
              {#each [...activeVersionList].reverse() as v}
                <li>
                  <button
                    class="flex items-center justify-between gap-2 {activeVersion === v.version ? 'bg-base-200 font-medium' : ''}"
                    onclick={() => { activeVersion = v.version; (document.activeElement as HTMLElement | null)?.blur(); }}
                  >
                    <span class="text-xs">{$t('chat.versionN', { values: { version: v.version } })}</span>
                    {#if v.time}<span class="text-xs text-base-content/50 font-mono">{v.time}</span>{/if}
                  </button>
                </li>
              {/each}
            {/if}
          </ul>
        </div>
      {:else}
        <span class="text-sm font-semibold flex-1 truncate">
          {paneView === 'flows' ? $t('nav.flows') : creationsTitle || $t('chat.work')}
        </span>
      {/if}
      {#if activeArtifact?.url && (activeArtifact.codeUrl || activeArtifact.url.endsWith('.html') || activeArtifact.url.endsWith('.md') || activeArtifact.url.endsWith('.txt'))}
        <div class="flex items-center rounded-md bg-base-200 p-0.5 shrink-0">
          <button
            class="py-0.5 px-2 rounded text-xs cursor-pointer border-none transition-colors {!viewSource ? 'bg-base-100 font-medium shadow-sm' : 'bg-transparent text-base-content/60 hover:text-base-content'}"
            onclick={() => viewSource = false}
          >{$t('chat.preview')}</button>
          <button
            class="py-0.5 px-2 rounded text-xs cursor-pointer border-none transition-colors {viewSource ? 'bg-base-100 font-medium shadow-sm' : 'bg-transparent text-base-content/60 hover:text-base-content'}"
            onclick={() => viewSource = true}
          >{$t('chat.artifactCode')}</button>
        </div>
      {/if}
      {#if activeArtifact && activeVersion != null && activeVersionList.length > 0 && activeArtifact.version < activeVersionList[activeVersionList.length - 1].version}
        <button
          class="py-1 px-2 rounded-md text-xs font-medium cursor-pointer bg-base-200 hover:bg-base-300 text-base-content/80 hover:text-base-content transition-colors shrink-0 border-none"
          onclick={() => { if (activeArtifact) onrestoreversion?.(activeArtifact.documentId, activeArtifact.version); activeVersion = null; }}
          title={$t('chat.makeVersionCurrent')}
        >{$t('chat.restore')}</button>
      {/if}
      {#if activeArtifact?.url}
        <div class="dropdown dropdown-end shrink-0">
          <button
            tabindex="0"
            class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors"
            title={$t('chat.downloadFile', { values: { title: activeArtifact.title } })}
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
          </button>
          <ul class="dropdown-content menu menu-sm z-30 mt-1 w-44 rounded-lg bg-base-100 border border-base-300 shadow-lg p-1">
            <li>
              <a
                href={backendUrl(activeArtifact.url)}
                download={activeArtifact.title}
                onclick={(e) => { downloadArtifact(e, activeArtifact?.url ?? '', activeArtifact?.title); (document.activeElement as HTMLElement | null)?.blur(); }}
                class="text-xs"
              >{$t('common.download')}</a>
            </li>
            <li>
              <button class="text-xs" onclick={() => { copyArtifact(); (document.activeElement as HTMLElement | null)?.blur(); }}>{$t('chat.copyContent')}</button>
            </li>
            <li>
              <button class="text-xs" onclick={() => { shareOpen = true; (document.activeElement as HTMLElement | null)?.blur(); }}>{$t('chat.share')}</button>
            </li>
          </ul>
        </div>
      {/if}
      <button
        class="max-md:hidden w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors shrink-0"
        onclick={() => (workFull = !workFull)}
        title={workFull ? $t('chat.exitFullView') : $t('chat.fullView')}
      >
        {#if workFull}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 14 10 14 10 20"/><polyline points="20 10 14 10 14 4"/><line x1="14" y1="10" x2="21" y2="3"/><line x1="3" y1="21" x2="10" y2="14"/></svg>
        {:else}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="15 3 21 3 21 9"/><polyline points="9 21 3 21 3 15"/><line x1="21" y1="3" x2="14" y2="10"/><line x1="3" y1="21" x2="10" y2="14"/></svg>
        {/if}
      </button>
      <button
        class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors shrink-0"
        onclick={closePane}
        title={$t('chat.closeWorkPanel')}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
      </button>
    </div>
    <!-- Creations content — one renderer for every format, routed by extension -->
    <div class="flex-1 overflow-y-auto">
      {#if paneView === 'flows'}
        {#if flowsPane}{@render flowsPane()}{/if}
      {:else if activeArtifact?.url}
        <!-- Key on documentId:version so a new version re-mounts the viewer in
             place (and the version-specific URL also defeats the browser cache). -->
        {#key `${activeArtifact.documentId}:${activeArtifact.version}:${viewSource}`}
          <WorkViewer
            url={activeArtifact.url}
            title={activeArtifact.title}
            renderHtml={renderMarkdown}
            oncontentclick={handleWorkMentionClick}
            sourceView={viewSource}
            codeUrl={activeArtifact.codeUrl}
          />
        {/key}
      {:else if documents.length > 0}
        <!-- No file selected yet: list each distinct document to pick from. -->
        <div class="p-3 flex flex-col gap-1.5">
          <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-1 pt-1 pb-2">{$t('chat.filesInThread')}</div>
          {#each documents as a}
            {@const ListIcon = artifactIcons[a.kind]}
            <button
              class="flex items-center gap-3 w-full p-3 rounded-xl border border-base-content/10 bg-base-200/30 hover:border-base-content/20 hover:bg-base-200/50 cursor-pointer transition-colors text-left"
              onclick={() => openArtifact(a.id)}
            >
              {#if ListIcon}<ListIcon class="w-4 h-4 text-base-content/50 shrink-0" />{/if}
              <div class="flex-1 min-w-0">
                <div class="text-sm font-medium truncate">{a.title}</div>
                <div class="text-xs text-base-content/50">{a.kind === 'code' ? $t('chat.artifactCode') : a.kind === 'table' ? $t('chat.artifactSpreadsheet') : a.kind === 'slides' ? $t('chat.artifactPresentation') : $t('chat.artifactDocument')}</div>
              </div>
            </button>
          {/each}
        </div>
      {:else}
        <div class="flex flex-col items-center justify-center h-full gap-3 text-base-content/50 p-6">
          <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="3" width="18" height="18" rx="2"/>
            <path d="M9 3v18"/>
            <path d="M14 9l3 3-3 3"/>
          </svg>
          <div class="text-sm font-medium">{$t('chat.nothingHereYet')}</div>
          <div class="text-xs text-center max-w-[220px]">{$t('chat.workEmptyDesc')}</div>
        </div>
      {/if}
    </div>
  </div>
{/if}

{#if lightboxUrl}
  <button
    type="button"
    class="fixed inset-0 z-[90] flex items-center justify-center bg-black/80 p-6 border-0 cursor-zoom-out"
    onclick={() => (lightboxUrl = null)}
    aria-label={$t('chat.closeImage')}
  >
    <img src={lightboxUrl} alt={$t('chat.fullSize')} class="max-w-full max-h-full rounded-lg object-contain" />
  </button>
{/if}

{#if activeArtifact?.url}
  <ShareArtifactModal bind:show={shareOpen} url={activeArtifact.url} title={activeArtifact.title} />
{/if}
</div>
