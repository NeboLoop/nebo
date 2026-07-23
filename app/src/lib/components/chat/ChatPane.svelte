<script lang="ts">
  import { t } from 'svelte-i18n';
  import ChatComposer from './ChatComposer.svelte';
  import WorkViewer from './WorkViewer.svelte';
  import ShareArtifactModal from './ShareArtifactModal.svelte';
  import AskWidget from './AskWidget.svelte';
  import type { AskWidgetDef } from './AskWidget.svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { downloadArtifact } from '$lib/chat/download';
  import { backendUrl } from '$lib/api/base';
  import { addToast } from '$lib/stores/toast';
  import { marked } from 'marked';
  import FileText from 'lucide-svelte/icons/file-text';
  import Code from 'lucide-svelte/icons/code';
  import Table from 'lucide-svelte/icons/table';
  import Presentation from 'lucide-svelte/icons/presentation';
  import type { UploadedAttachment } from '$lib/types/attachment';
  import { getAttachmentType, formatFileSize } from '$lib/types/attachment';

  // Configure marked for streaming-friendly rendering
  marked.setOptions({
    breaks: true,
    gfm: true,
  });

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
  interface ToolMsg {
    name: string;
    status: string;
    request: Record<string, unknown>;
    response: string;
    statusText?: string;
    label?: string;     // human activity label (gerund), from the start phase
    outcome?: string;   // past-tense outcome, from the result phase
    durationMs?: number;
  }

  type Message =
    | { type: 'user'; content: string; time?: string; attachments?: UploadedAttachment[] }
    | { type: 'thinking'; content: string; duration: string }
    | { type: 'ask'; requestId: string; prompt: string; widgets: AskWidgetDef[]; response?: string }
    | { type: 'assistant'; content: string; html?: string; time?: string; delegateAgentId?: string; delegateAgentName?: string; id?: string; attachments?: UploadedAttachment[]; tools?: ToolMsg[]; streaming?: boolean };

  type AgentInfo = { id: string; name: string; color: string; initial: string; role: string; status: string; isApp?: boolean };

  let { messages = [], agentName = 'Agent', agentId = '', threadId = '', sessionId = '', headerTitle = '', headerRight = '', placeholder = '', emptyIcon = '', emptyTitle = '', emptyDesc = '', allAgents = [], activityStatus = '', tokenUsage = null, quotaWarning = '', chatError = '', onsend, onstop, onedit, onredo, onasksubmit, onrestoreversion, ondismisswarning, ondismisserror, onloadmore, isLoading = false, isLoadingMore = false, hasMore = false, allowAttachments = true }: {
    messages?: Message[];
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
    tokenUsage?: { input: number; output: number; cacheRead?: number; cacheCreation?: number; overhead?: number } | null;
    quotaWarning?: string;
    chatError?: string;
    onsend?: (text: string, files: { file: File; id: string; previewUrl: string | null; isImage: boolean }[]) => void;
    onstop?: () => void;
    onedit?: (msgIndex: number, newContent: string) => void;
    onredo?: (msgIndex: number) => void;
    onasksubmit?: (requestId: string, value: string) => void;
    onrestoreversion?: (documentId: string, version: number) => void;
    ondismisswarning?: () => void;
    ondismisserror?: () => void;
    onloadmore?: () => void;
    isLoading?: boolean;
    isLoadingMore?: boolean;
    hasMore?: boolean;
    /** Hide the attach affordance when the chat's send pathway ignores files. */
    allowAttachments?: boolean;
  } = $props();

  let composerRef = $state<{ focus: () => void; focusAndInsert: (char: string) => void; addFiles: (files: File[]) => void } | null>(null);
  let creationsOpen = $state(false);
  // Empty = default panel title ($t('chat.work') at render time).
  let creationsTitle = $state('');
  let activeArtifactId = $state<string | null>(null);
  // Pinned version of the active document; null = follow the latest version.
  let activeVersion = $state<number | null>(null);

  // Replace <@id> tokens (already HTML-escaped) with styled mention chips
  function renderMentionChips(escapedHtml: string): string {
    return escapedHtml.replace(/&lt;@([a-zA-Z0-9._-]+)&gt;/g, (_, id) => {
      const agent = allAgents.find(a => a.id === id);
      if (!agent) return `<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium bg-base-300 text-base-content/70 align-baseline">@unknown</span>`;
      const c = AGENT_COLORS_MAP[agent.color || 'teal'] || AGENT_COLORS_MAP['teal'];
      return `<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium align-baseline ${c.bgClass} ${c.inkClass}"><span class="w-4 h-4 rounded-sm flex items-center justify-center text-xs font-semibold shrink-0">${agent.initial || agent.name.charAt(0).toUpperCase()}</span><span>${agent.name}</span></span>`;
    });
  }

  // Render assistant message content with basic markdown + mention chips.
  // Code blocks get a copy affordance: each <pre> is wrapped with a positioned
  // button handled by delegated click (copyCodeBlock) — the button copies the
  // wrapped <code>'s text, so no payload attributes are needed.
  function renderMarkdown(content: string): string {
    if (!content) return '';
    const html = marked.parse(content, { async: false }) as string;
    const withCopy = html
      .replace(
        /<pre>/g,
        `<div class="relative group/code"><button type="button" data-code-copy title="${$t('chat.copyCode')}" class="absolute top-2 right-2 z-10 px-2 py-0.5 rounded text-xs font-medium bg-base-100/80 border border-base-content/10 text-base-content/60 opacity-0 group-hover/code:opacity-100 hover:text-base-content hover:bg-base-200 cursor-pointer transition-opacity">${$t('common.copy')}</button><pre>`
      )
      .replace(/<\/pre>/g, '</pre></div>');
    return renderMentionChips(withCopy);
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
        const res = await fetch(src);
        if (!res.ok) throw new Error(`${res.status}`);
        await navigator.clipboard.writeText(await res.text());
      } else {
        await navigator.clipboard.writeText(new URL(src, window.location.origin).href);
      }
      addToast($t('chat.copied'), 'success');
    } catch {
      addToast($t('chat.copyFailed'), 'error');
    }
  }

  function openArtifact(id: string) {
    activeArtifactId = id;
    activeVersion = null; // follow latest; the version dropdown pins an older one
    viewSource = false;
    openWorkPanel();
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
  let userResized = $state(false);
  let resizing = $state(false);
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
      if (creationsOpen) creationsWidth = clampPanelWidth(creationsWidth);
    });
    ro.observe(containerEl);
    return () => ro.disconnect();
  });

  // Open the panel at HALF the chat area (Claude-style) unless the user has
  // dragged it to their own width this session; always resizable after.
  // Opening with nothing selected shows the artifact list in the panel —
  // never auto-pick a file the user didn't ask for.
  function openWorkPanel() {
    creationsOpen = true;
    if (activeArtifactId && !documentVersions.has(activeArtifactId)) {
      activeArtifactId = null; // stale selection from another thread
      activeVersion = null;
    }
    if (!userResized && containerEl) {
      const w = containerEl.getBoundingClientRect().width;
      creationsWidth = clampPanelWidth(Math.max(360, w * 0.5));
    }
  }

  function startResize(e: MouseEvent) {
    e.preventDefault();
    resizing = true;
    const onMove = (ev: MouseEvent) => {
      if (!containerEl) return;
      const rect = containerEl.getBoundingClientRect();
      creationsWidth = clampPanelWidth(rect.right - ev.clientX);
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
    if (document.activeElement?.tagName === 'INPUT' || document.activeElement?.tagName === 'TEXTAREA') return;
    if ((document.activeElement as HTMLElement)?.isContentEditable) return;
    if (e.ctrlKey || e.metaKey || e.altKey || e.key.length > 1) return;
    if (document.querySelector('[data-modal-open]')) return;
    e.preventDefault();
    composerRef?.focusAndInsert(e.key);
  }

  export function showCreations(title = '') {
    creationsTitle = title;
    openWorkPanel();
  }

  export function hideCreations() {
    creationsOpen = false;
  }

  const hasMessages = $derived(messages.length > 0);

  // Scroll state
  let messagesContainer = $state<HTMLDivElement | null>(null);
  let messagesContent = $state<HTMLDivElement | null>(null);
  let showScrollButton = $state(false);
  let autoScrollEnabled = $state(true);
  let scrollingProgrammatically = false;
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
    const key = last ? `${users.length}:${last.content}` : null;
    if (key === lastUserMsgKey || key === null) return;
    lastUserMsgKey = key;
    if (!initialScrollDone) return; // opening an old chat is not a send
    requestAnimationFrame(() => {
      const scroller = messagesContainer;
      const content = messagesContent;
      if (!scroller || !content) return;
      updateTurnSpacer();
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

  // Preserve scroll position after older messages are prepended
  $effect(() => {
    if (isLoadingMore && messagesContainer) {
      prevScrollHeight = messagesContainer.scrollHeight;
    }
  });
  $effect(() => {
    // When loading finishes and messages have been prepended, adjust scroll
    if (!isLoadingMore && prevScrollHeight > 0 && messagesContainer) {
      scrollingProgrammatically = true;
      requestAnimationFrame(() => {
        if (messagesContainer) {
          const added = messagesContainer.scrollHeight - prevScrollHeight;
          messagesContainer.scrollTop += added;
        }
        prevScrollHeight = 0;
        requestAnimationFrame(() => { scrollingProgrammatically = false; });
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
          });
          return;
        }
        requestAnimationFrame(pin);
      };
      requestAnimationFrame(pin);
    }
  });

  function handleScroll() {
    if (!messagesContainer || scrollingProgrammatically) return;
    const { scrollTop, scrollHeight, clientHeight } = messagesContainer;
    const distanceFromBottom = scrollHeight - scrollTop - clientHeight;
    const scrolledUp = scrollTop < lastScrollTop - 1;
    lastScrollTop = scrollTop;
    showScrollButton = distanceFromBottom > 100;

    // Disengage only on genuine user intent — an UPWARD scroll away from the
    // bottom. Position alone can't distinguish "user scrolled away" from a
    // pin that hasn't caught up with fresh content yet.
    if (scrolledUp && distanceFromBottom > 100) {
      autoScrollEnabled = false;
    } else if (distanceFromBottom <= 100) {
      autoScrollEnabled = true;
    }

    // Load older messages when scrolled near top
    if (scrollTop < 100 && hasMore && !isLoadingMore && onloadmore) {
      onloadmore();
    }
  }

  function scrollToBottom() {
    if (messagesContainer) {
      scrollingProgrammatically = true;
      messagesContainer.scrollTo({ top: messagesContainer.scrollHeight, behavior: 'smooth' });
      showScrollButton = false;
      autoScrollEnabled = true;
      requestAnimationFrame(() => {
        requestAnimationFrame(() => { scrollingProgrammatically = false; });
      });
    }
  }

  // Dropzone state
  let isDragging = $state(false);
  let dragCounter = $state(0);

  // Tool timeline collapse state, keyed by the owning reply's id (stable across
  // re-renders — index keys would drift as new messages stream in).
  let collapsedToolGroups = $state<Record<string, boolean>>({});
  function toggleToolGroup(key: string) {
    collapsedToolGroups[key] = !collapsedToolGroups[key];
  }

  // Individual tool result expand state
  let expandedResults = $state<Record<string, boolean>>({});
  function toggleResult(key: string) {
    expandedResults[key] = !expandedResults[key];
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
  function stepOutcome(tool: ToolMsg): string {
    return tool.outcome ?? tool.label ?? $t('chat.usedTool', { values: { name: tool.name } });
  }
  // Correct tool signature: MCP → "slug · tool", STRAP → "name · resource.action".
  function strapSig(t: ToolMsg): string {
    if (t.name.startsWith('mcp__')) {
      return t.name.slice(5).replace('__', ' · ').replaceAll('_', ' ');
    }
    const req = t.request as { resource?: string; action?: string } | undefined;
    if (req?.resource && req?.action) return `${t.name} · ${req.resource}.${req.action}`;
    return t.name;
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

  <!-- Header -->
  {#if headerTitle}
    <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
      <span class="text-sm font-semibold truncate min-w-0">{headerTitle}</span>
      {#if headerRight}
        <button
          data-tour="work"
          class="text-sm ml-auto shrink-0 whitespace-nowrap cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors flex items-center gap-1.5"
          onclick={() => creationsOpen ? (creationsOpen = false) : openWorkPanel()}
          title={creationsOpen ? $t('chat.closeWorkPanel') : $t('chat.openWorkPanel')}
        >
          {headerRight}
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="{creationsOpen ? 'text-primary' : ''}">
            <rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/><path d="M14 9l3 3-3 3"/>
          </svg>
        </button>
      {/if}
    </div>
  {/if}

  <!-- Messages / Empty state -->
  {#if !hasMessages && emptyTitle}
    <div class="flex-1 flex flex-col items-center justify-center gap-4 p-6">
      {#if emptyIcon}
        <div class="w-12 h-12 rounded-box flex items-center justify-center font-mono text-xl font-semibold bg-primary text-primary-content">{emptyIcon}</div>
        <div class="text-base font-semibold">{emptyTitle}</div>
      {:else}
        <div class="text-2xl font-semibold text-base-content">{emptyTitle}</div>
      {/if}
      {#if emptyDesc}
        <div class="text-sm text-base-content/50 text-center max-w-[320px] leading-relaxed">{emptyDesc}</div>
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
  <div bind:this={messagesContainer} onscroll={handleScroll} class="h-full overflow-y-auto p-[18px_24px]">
  <div bind:this={messagesContent} class="max-w-3xl mx-auto flex flex-col gap-1" data-selectable>
    {#if isLoadingMore}
      <div class="flex justify-center py-3">
        <div class="loading loading-spinner loading-sm text-base-content/30"></div>
      </div>
    {/if}

    <!-- Tool timeline for one reply ("Used N tools"), rendered inside the assistant
         message that ran the tools so they can never detach. keyId = reply id. -->
    {#snippet toolTimeline(tools: ToolMsg[], keyId: string)}
      {@const hasRunning = tools.some((t: ToolMsg) => t.status === 'running')}
      {@const isOpen = !!collapsedToolGroups[keyId]}
      <div class="max-w-[640px] my-1">
        <button
          class="flex items-center gap-1.5 text-xs text-base-content/50 cursor-pointer bg-transparent border-none p-0 hover:text-base-content/70 transition-colors"
          onclick={() => toggleToolGroup(keyId)}
        >
          {#if hasRunning}
            <svg width="14" height="14" viewBox="0 0 14 14" class="animate-spin text-primary shrink-0"><circle cx="7" cy="7" r="5.5" stroke="currentColor" stroke-width="1.5" fill="none" stroke-dasharray="20 14" stroke-linecap="round"/></svg>
            <span class="text-xs text-base-content/70 truncate max-w-[60vw] md:max-w-md">{workLineLabel(tools)}</span>
          {:else}
            {@const wd = workLineDuration(tools)}
            <svg width="13" height="13" viewBox="0 0 18 18" fill="none" class="text-base-content/50 shrink-0"><path d="M10.5 3.5C10.5 2.67 11.17 2 12 2C12.5 2 13.09 2.24 13.45 2.59L15.41 4.55C15.76 4.91 16 5.5 16 6C16 6.83 15.33 7.5 14.5 7.5C14.16 7.5 13.85 7.38 13.6 7.18L12.18 8.6C12.38 8.85 12.5 9.16 12.5 9.5C12.5 10.33 11.83 11 11 11C10.67 11 10.36 10.88 10.11 10.69L5.69 15.11C5.5 15.3 5.25 15.41 5 15.41C4.75 15.41 4.5 15.3 4.31 15.11L2.89 13.69C2.7 13.5 2.59 13.25 2.59 13C2.59 12.75 2.7 12.5 2.89 12.31L7.31 7.89C7.12 7.64 7 7.33 7 7C7 6.17 7.67 5.5 8.5 5.5C8.84 5.5 9.15 5.62 9.4 5.82L10.82 4.4C10.62 4.15 10.5 3.84 10.5 3.5Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/></svg>
            <span class="text-xs truncate max-w-[60vw] md:max-w-md">{workLineLabel(tools)}</span>
            {#if wd}<span class="text-xs text-base-content/40">· {wd}</span>{/if}
            <span class="text-xs transition-transform {isOpen ? 'rotate-180' : ''}">&darr;</span>
          {/if}
        </button>

        {#if isOpen}
          <div class="mt-2 ml-1 flex flex-col">
            {#each tools as tool, tidx}
              {@const resultKey = `${keyId}-${tidx}`}
              {@const isExpanded = expandedResults[resultKey]}
              <div class="flex items-start gap-2.5">
                <div class="flex flex-col items-center shrink-0 w-5">
                  {#if tool.status === 'running'}
                    <svg width="18" height="18" viewBox="0 0 18 18" class="text-primary shrink-0 animate-spin"><circle cx="9" cy="9" r="6" stroke="currentColor" stroke-width="1.5" fill="none" stroke-dasharray="22 16" stroke-linecap="round"/></svg>
                  {:else}
                    <svg width="18" height="18" viewBox="0 0 18 18" fill="none" class="text-base-content shrink-0">
                      <path d="M10.5 3.5C10.5 2.67 11.17 2 12 2C12.5 2 13.09 2.24 13.45 2.59L15.41 4.55C15.76 4.91 16 5.5 16 6C16 6.83 15.33 7.5 14.5 7.5C14.16 7.5 13.85 7.38 13.6 7.18L12.18 8.6C12.38 8.85 12.5 9.16 12.5 9.5C12.5 10.33 11.83 11 11 11C10.67 11 10.36 10.88 10.11 10.69L5.69 15.11C5.5 15.3 5.25 15.41 5 15.41C4.75 15.41 4.5 15.3 4.31 15.11L2.89 13.69C2.7 13.5 2.59 13.25 2.59 13C2.59 12.75 2.7 12.5 2.89 12.31L7.31 7.89C7.12 7.64 7 7.33 7 7C7 6.17 7.67 5.5 8.5 5.5C8.84 5.5 9.15 5.62 9.4 5.82L10.82 4.4C10.62 4.15 10.5 3.84 10.5 3.5Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round"/>
                    </svg>
                  {/if}
                  {#if tidx < tools.length - 1 || isExpanded}
                    <div class="w-px flex-1 min-h-[28px] bg-base-300"></div>
                  {/if}
                </div>
                <div class="flex-1 min-w-0 pb-3">
                  <div class="flex items-baseline gap-2 text-xs">
                    <span class="truncate {tool.status === 'running' ? 'text-base-content/70' : ''}">{tool.status === 'running' ? (tool.label ?? tool.name) : stepOutcome(tool)}{#if tool.status === 'running' && tool.statusText}<span class="text-base-content/50 ml-1">{tool.statusText}</span>{/if}</span>
                    <span class="font-mono text-base-content/40 shrink-0">{strapSig(tool)}</span>
                    {#if tool.durationMs}<span class="text-base-content/40 shrink-0">{fmtDuration(tool.durationMs)}</span>{/if}
                  </div>
                  {#if tool.status !== 'running'}
                    {#if isExpanded}
                      <div class="mt-2 rounded-lg border border-base-300 bg-base-100 overflow-hidden">
                        <div class="px-3.5 pt-3 pb-2">
                          <div class="text-xs font-semibold mb-1.5">{$t('chat.request')}</div>
                          <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap">{JSON.stringify(tool.request, null, 2)}</pre>
                        </div>
                        <div class="px-3.5 pt-2 pb-3 border-t border-base-300">
                          <div class="text-xs font-semibold mb-1.5">{$t('chat.response')}</div>
                          <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap">{tool.response}</pre>
                        </div>
                      </div>
                      <button
                        class="mt-1.5 py-0.5 px-2 rounded text-xs font-medium bg-base-200 cursor-pointer border-none hover:bg-base-300 transition-colors"
                        onclick={() => toggleResult(resultKey)}
                      >{$t('chat.hide')}</button>
                    {:else}
                      <div class="mt-1">
                        <button
                          class="py-0.5 px-2 rounded text-xs font-medium cursor-pointer border-none transition-colors {tool.status === 'success' ? 'bg-base-200 hover:bg-base-300' : 'bg-error/10 text-error hover:bg-error/20'}"
                          onclick={() => toggleResult(resultKey)}
                        >{$t('chat.result')}</button>
                      </div>
                    {/if}
                  {/if}
                </div>
              </div>
            {/each}
            {#if !hasRunning}
              <div class="flex items-center gap-2.5">
                <div class="flex items-center justify-center w-5 shrink-0">
                  <svg width="18" height="18" viewBox="0 0 18 18" fill="none" class="text-base-content">
                    <circle cx="9" cy="9" r="7" stroke="currentColor" stroke-width="1.2"/>
                    <path d="M6 9L8.25 11.25L12.25 6.75" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                </div>
                <span class="text-xs">{$t('common.done')}</span>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    {/snippet}

    {#each groupedMessages as msg, idx}
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
          <div class="max-w-[640px] self-end mt-3" data-user-msg>
            <div class="py-2.5 px-3.5 rounded-xl text-sm leading-relaxed bg-base-200 rounded-br-sm prose prose-sm max-w-none [&_p]:my-0 [&_ul]:my-1 [&_ol]:my-1 [&>:first-child]:mt-0 [&>:last-child]:mb-0">
              {@html renderMarkdown(msg.content)}
              {#if msg.attachments?.length}
                <div class="flex flex-wrap gap-2 mt-2">
                  {#each msg.attachments as att}
                    {@const attType = getAttachmentType(att.mimeType)}
                    {#if attType === 'image'}
                      <button type="button" class="block p-0 bg-transparent border-0 cursor-zoom-in" onclick={() => (lightboxUrl = att.url)} aria-label={$t('chat.viewImage')}>
                        <img
                          src={att.thumbnailUrl || att.url}
                          alt={att.filename}
                          class="max-w-[240px] max-h-[180px] rounded-lg border border-base-content/15 object-cover"
                          loading="lazy"
                        />
                      </button>
                    {:else if attType === 'video'}
                      <video
                        src={att.url}
                        controls
                        preload="metadata"
                        class="max-w-[320px] max-h-[240px] rounded-lg border border-base-content/15"
                      >
                        <track kind="captions" />
                      </video>
                    {:else}
                      <a
                        href={att.url}
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
            <div class="flex items-center gap-1 justify-end mt-1.5">
              {#if msg.time}
                <span class="text-xs text-base-content/50 font-mono mr-1">{msg.time}</span>
              {/if}
              <button
                class="w-7 h-7 rounded-md grid place-items-center text-base-content/50 hover:text-base-content hover:bg-base-200 cursor-pointer bg-transparent border-none transition-colors"
                title={$t('chat.editResend')}
                onclick={() => startEdit(origIdx, msg.content)}
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
              </button>
              <button
                class="w-7 h-7 rounded-md grid place-items-center {copiedIdx === origIdx ? 'text-success' : 'text-base-content/50 hover:text-base-content hover:bg-base-200'} cursor-pointer bg-transparent border-none transition-colors"
                title={copiedIdx === origIdx ? $t('chat.copied') : $t('common.copy')}
                onclick={() => copyMessage(msg.content, origIdx)}
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
            disabled={!isLoading}
            onSubmit={(id, val) => onasksubmit?.(id, val)}
          />
        </div>

      {:else if msg.type === 'assistant'}
        {@const origIdx = originalIndices[idx]}
        {@const nextGroup = groupedMessages[idx + 1]}
        <!-- One assistant TURN reads as one container: narration segments
             between tool groups flow as paragraphs; the time/copy/retry row
             renders once, on the segment that ends the turn. -->
        {@const isTurnEnd = nextGroup ? (nextGroup.type === 'user' || nextGroup.type === 'ask') : !isLoading}
        {@const isTurnStart = idx === 0 || groupedMessages[idx - 1]?.type === 'user' || groupedMessages[idx - 1]?.type === 'ask'}
        <div class="max-w-[640px] {isTurnStart ? 'mt-3' : 'mt-1.5'}">
          {#if msg.delegateAgentName}
            {@const da = allAgents.find(a => a.id === msg.delegateAgentId)}
            {@const dc = AGENT_COLORS_MAP[da?.color || 'teal'] || AGENT_COLORS_MAP['teal']}
            <div class="flex items-center gap-1.5 mb-1">
              <div class="w-5 h-5 rounded-md flex items-center justify-center text-xs font-semibold {dc.bgClass} {dc.inkClass}">
                {da?.initial || msg.delegateAgentName.charAt(0).toUpperCase()}
              </div>
              <span class="text-xs font-medium">{msg.delegateAgentName}</span>
            </div>
          {/if}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="text-sm leading-relaxed prose prose-sm max-w-none" onclick={handleWorkMentionClick}>
            {#if msg.html}
              {@html linkWorkMentions(renderMentionChips(msg.html), (msg as any).workItems)}
            {:else}
              {@html linkWorkMentions(renderMarkdown(msg.content), (msg as any).workItems)}
            {/if}
          </div>
          <!-- Tools this reply ran, on the message itself — never a detached sibling. -->
          {#if msg.tools?.length}
            {@render toolTimeline(msg.tools, msg.id ?? `m${origIdx}`)}
          {/if}
          {#if msg.attachments?.length}
            <div class="flex flex-wrap gap-2 mt-2">
              {#each msg.attachments as att}
                {@const attType = getAttachmentType(att.mimeType)}
                {#if attType === 'image'}
                  <button type="button" class="block p-0 bg-transparent border-0 cursor-zoom-in" onclick={() => (lightboxUrl = att.url)} aria-label={$t('chat.viewImage')}>
                    <img
                      src={att.thumbnailUrl || att.url}
                      alt={att.filename}
                      class="max-w-[240px] max-h-[180px] rounded-lg border border-base-content/15 object-cover"
                      loading="lazy"
                    />
                  </button>
                {:else if attType === 'video'}
                  <video
                    src={att.url}
                    controls
                    preload="metadata"
                    class="max-w-[320px] max-h-[240px] rounded-lg border border-base-content/15"
                  >
                    <track kind="captions" />
                  </video>
                {:else}
                  <a
                    href={att.url}
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
          {#each artifacts.filter(a => a.messageId === msg.id) as artifact}
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
              {#if msg.time}
                <span class="text-xs text-base-content/50 font-mono mr-1">{msg.time}</span>
              {/if}
              <button
                class="w-7 h-7 rounded-md grid place-items-center {copiedIdx === origIdx ? 'text-success' : 'text-base-content/50 hover:text-base-content hover:bg-base-200'} cursor-pointer bg-transparent border-none transition-colors"
                title={copiedIdx === origIdx ? $t('chat.copied') : $t('common.copy')}
                onclick={() => copyMessage(msg.content, origIdx)}
              >
                {#if copiedIdx === origIdx}
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
    {/each}

    {#if isLoading && groupedMessages.length > 0 && groupedMessages[groupedMessages.length - 1]?.type !== 'assistant'}
      <div class="max-w-[640px] mt-3 py-2 flex items-center gap-2">
        <span class="loading loading-spinner loading-xs text-primary"></span>
        <span class="text-sm text-base-content/50 animate-pulse">{activityStatus || $t('chat.working')}</span>
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

  <!-- Token usage badge -->
  {#if tokenUsage}
    {@const totalPrompt = tokenUsage.input + (tokenUsage.cacheRead ?? 0) + (tokenUsage.cacheCreation ?? 0)}
    {@const conversationIn = Math.max(0, totalPrompt - (tokenUsage.overhead ?? 0))}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-6 pb-1">
      <span class="text-xs text-base-content/50 font-mono" title={$t('chat.tokenTooltip', { values: { total: totalPrompt.toLocaleString(), overhead: (tokenUsage.overhead ?? 0).toLocaleString(), cacheRead: (tokenUsage.cacheRead ?? 0).toLocaleString() } })}>
        {$t('chat.tokensInOut', { values: { input: conversationIn.toLocaleString(), output: tokenUsage.output.toLocaleString() } })}
      </span>
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
            <a href="/pricing" class="btn btn-primary btn-xs">{$t('chat.viewPlans')}</a>
          {/if}
          <button class="btn btn-ghost btn-xs" onclick={() => ondismisserror?.()}>x</button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Composer -->
  <div class="max-w-3xl mx-auto w-full shrink-0">
    <ChatComposer
      {agentName}
      {agentId}
      {threadId}
      {sessionId}
      {placeholder}
      {allAgents}
      {onsend}
      {onstop}
      {isLoading}
      {allowAttachments}
      bind:this={composerRef}
    />
  </div>
</div>

<!-- Resize handle + Creations panel -->
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
  <div class="flex flex-col bg-base-100 min-h-0 min-w-0 overflow-hidden shrink-0 border-l border-base-300 max-md:fixed max-md:inset-0 max-md:z-[70] max-md:!w-full max-md:border-l-0 {resizing ? 'pointer-events-none' : ''}" style="width: {creationsWidth}px">
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
        <span class="text-sm font-semibold flex-1 truncate">{creationsTitle || $t('chat.work')}</span>
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
                href={activeArtifact.url}
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
        class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors shrink-0"
        onclick={() => creationsOpen = false}
        title={$t('chat.closeWorkPanel')}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
      </button>
    </div>
    <!-- Creations content — one renderer for every format, routed by extension -->
    <div class="flex-1 overflow-y-auto">
      {#if activeArtifact?.url}
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
    class="fixed inset-0 z-[80] flex items-center justify-center bg-black/80 p-6 border-0 cursor-zoom-out"
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
