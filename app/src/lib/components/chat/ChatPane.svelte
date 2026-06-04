<script lang="ts">
  import ChatComposer from './ChatComposer.svelte';
  import AskWidget from './AskWidget.svelte';
  import type { AskWidgetDef } from './AskWidget.svelte';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
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
    id: string;
    messageId?: string;
    title: string;
    kind: 'document' | 'code' | 'table' | 'slides';
    preview: string;
  }

  interface ToolMsg {
    type: 'tool';
    name: string;
    status: string;
    duration: string;
    request: Record<string, unknown>;
    response: string;
    statusText?: string;
  }

  interface ToolGroup {
    type: 'tool-group';
    tools: ToolMsg[];
  }

  type Message =
    | { type: 'user'; content: string; time?: string; attachments?: UploadedAttachment[] }
    | { type: 'thinking'; content: string; duration: string }
    | ToolMsg
    | ToolGroup
    | { type: 'ask'; requestId: string; prompt: string; widgets: AskWidgetDef[]; response?: string }
    | { type: 'assistant'; content: string; html?: string; time?: string; delegateAgentId?: string; delegateAgentName?: string; id?: string; attachments?: UploadedAttachment[] };

  type AgentInfo = { id: string; name: string; color: string; initial: string; role: string; status: string; isApp?: boolean };

  let { messages = [], agentName = 'Agent', agentId = '', threadId = '', sessionId = '', headerTitle = '', headerRight = '', placeholder = '', emptyIcon = '', emptyTitle = '', emptyDesc = '', allAgents = [], followupSuggestions = [], activityStatus = '', tokenUsage = null, quotaWarning = '', onsend, onstop, onedit, onredo, onasksubmit, onfollowupselect, ondismissfollowups, ondismisswarning, onloadmore, isLoading = false, isLoadingMore = false, hasMore = false }: {
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
    followupSuggestions?: string[];
    activityStatus?: string;
    tokenUsage?: { input: number; output: number; cacheRead?: number; cacheCreation?: number } | null;
    quotaWarning?: string;
    onsend?: (text: string, files: { file: File; id: string; previewUrl: string | null; isImage: boolean }[]) => void;
    onstop?: () => void;
    onedit?: (msgIndex: number, newContent: string) => void;
    onredo?: (msgIndex: number) => void;
    onasksubmit?: (requestId: string, value: string) => void;
    onfollowupselect?: (suggestion: string) => void;
    ondismissfollowups?: () => void;
    ondismisswarning?: () => void;
    onloadmore?: () => void;
    isLoading?: boolean;
    isLoadingMore?: boolean;
    hasMore?: boolean;
  } = $props();

  let composerRef = $state<{ focus: () => void; focusAndInsert: (char: string) => void; addFiles: (files: File[]) => void } | null>(null);
  let creationsOpen = $state(false);
  let creationsTitle = $state('Work');
  let activeArtifactId = $state<string | null>(null);

  // Replace <@id> tokens (already HTML-escaped) with styled mention chips
  function renderMentionChips(escapedHtml: string): string {
    return escapedHtml.replace(/&lt;@([a-zA-Z0-9._-]+)&gt;/g, (_, id) => {
      const agent = allAgents.find(a => a.id === id);
      if (!agent) return `<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium bg-base-300 text-base-content/70 align-baseline">@unknown</span>`;
      const c = AGENT_COLORS_MAP[agent.color || 'teal'] || AGENT_COLORS_MAP['teal'];
      return `<span class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium align-baseline ${c.bgClass} ${c.inkClass}"><span class="w-4 h-4 rounded-sm flex items-center justify-center text-xs font-semibold shrink-0">${agent.initial || agent.name.charAt(0).toUpperCase()}</span><span>${agent.name}</span></span>`;
    });
  }

  // Render user message content with mention chips
  function renderUserContent(content: string): string {
    const escaped = content
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;');
    return renderMentionChips(escaped);
  }

  // Render assistant message content with basic markdown + mention chips
  function renderMarkdown(content: string): string {
    if (!content) return '';
    const html = marked.parse(content, { async: false }) as string;
    return renderMentionChips(html);
  }

  // Artifacts will be populated by agent tool results in the future
  const artifacts: Artifact[] = [];

  const artifactIcons = { document: FileText, code: Code, table: Table, slides: Presentation };
  const activeArtifact = $derived(artifacts.find(a => a.id === activeArtifactId));

  function openArtifact(id: string) {
    activeArtifactId = id;
    creationsOpen = true;
    const a = artifacts.find(x => x.id === id);
    if (a) creationsTitle = a.title;
  }
  const CREATIONS_MIN = 220;
  let creationsWidth = $state(CREATIONS_MIN);
  let resizing = $state(false);
  let containerEl = $state<HTMLDivElement | null>(null);

  function startResize(e: MouseEvent) {
    e.preventDefault();
    resizing = true;
    const onMove = (ev: MouseEvent) => {
      if (!containerEl) return;
      const rect = containerEl.getBoundingClientRect();
      const newWidth = rect.right - ev.clientX;
      const maxWidth = rect.width * 0.6;
      creationsWidth = Math.max(CREATIONS_MIN, Math.min(maxWidth, newWidth));
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

  export function showCreations(title = 'Work') {
    creationsTitle = title;
    creationsOpen = true;
  }

  export function hideCreations() {
    creationsOpen = false;
  }

  const hasMessages = $derived(messages.length > 0);

  // Scroll state
  let messagesContainer = $state<HTMLDivElement | null>(null);
  let showScrollButton = $state(false);
  let autoScrollEnabled = $state(true);
  let scrollingProgrammatically = false;
  let pendingScrollRAF: number | null = null;
  let initialScrollDone = false;
  let prevScrollHeight = 0;

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

  // Auto-scroll when messages change
  $effect(() => {
    const _count = messages.length; // track dependency
    if (!initialScrollDone) return;
    if (messagesContainer && autoScrollEnabled) {
      if (pendingScrollRAF) cancelAnimationFrame(pendingScrollRAF);
      scrollingProgrammatically = true;
      pendingScrollRAF = requestAnimationFrame(() => {
        pendingScrollRAF = null;
        if (messagesContainer && autoScrollEnabled) {
          messagesContainer.scrollTo({ top: messagesContainer.scrollHeight, behavior: 'smooth' });
        }
        requestAnimationFrame(() => { scrollingProgrammatically = false; });
      });
    }
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
    const wasNearBottom = !showScrollButton;
    showScrollButton = distanceFromBottom > 100;

    if (wasNearBottom && showScrollButton) {
      autoScrollEnabled = false;
    } else if (!wasNearBottom && !showScrollButton) {
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

  // Tool group collapse state
  let collapsedToolGroups = $state<Record<number, boolean>>({});
  function toggleToolGroup(idx: number) {
    collapsedToolGroups[idx] = !collapsedToolGroups[idx];
  }

  // Individual tool result expand state
  let expandedResults = $state<Record<string, boolean>>({});
  function toggleResult(key: string) {
    expandedResults[key] = !expandedResults[key];
  }

  // Group consecutive tool messages together, track original indices
  const groupedResult = $derived.by(() => {
    const groups: Message[] = [];
    const indices: number[] = [];
    let i = 0;
    while (i < messages.length) {
      const msg = messages[i];
      if (msg.type === 'tool') {
        const tools: ToolMsg[] = [];
        const firstIdx = i;
        while (i < messages.length && messages[i].type === 'tool') {
          tools.push(messages[i] as ToolMsg);
          i++;
        }
        groups.push({ type: 'tool-group', tools });
        indices.push(firstIdx);
      } else {
        groups.push(msg);
        indices.push(i);
        i++;
      }
    }
    return { groups, indices };
  });
  const groupedMessages = $derived(groupedResult.groups);
  const originalIndices = $derived(groupedResult.indices);

  // Drag-and-drop handlers
  function handleDragEnter(e: DragEvent) {
    e.preventDefault();
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

<div class="flex-1 flex min-w-0 min-h-0 overflow-hidden {resizing ? 'select-none' : ''}" bind:this={containerEl}>
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
      <div class="text-primary font-medium text-sm">Drop files here</div>
    </div>
  {/if}

  <!-- Header -->
  {#if headerTitle}
    <div class="h-11 px-[18px] border-b border-base-content/10 flex items-center gap-2 shrink-0">
      <span class="text-sm font-semibold">{headerTitle}</span>
      {#if headerRight}
        <button
          class="text-sm ml-auto cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors flex items-center gap-1.5"
          onclick={() => creationsOpen = !creationsOpen}
          title={creationsOpen ? 'Close Work panel' : 'Open Work panel'}
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
          title="Scroll to bottom"
        >
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="5" x2="12" y2="19"/><polyline points="19 12 12 19 5 12"/></svg>
        </button>
      </div>
    {/if}
  <div bind:this={messagesContainer} onscroll={handleScroll} class="h-full overflow-y-auto p-[18px_24px]">
  <div class="max-w-3xl mx-auto flex flex-col gap-1" data-selectable>
    {#if isLoadingMore}
      <div class="flex justify-center py-3">
        <div class="loading loading-spinner loading-sm text-base-content/30"></div>
      </div>
    {/if}
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
                <span class="text-xs text-base-content/50">Enter to submit · Esc to cancel</span>
                <div class="flex items-center gap-2">
                  <button
                    class="py-1.5 px-3 rounded-lg text-xs cursor-pointer border border-base-300 bg-transparent hover:bg-base-200 transition-colors"
                    onclick={cancelEdit}
                  >Cancel</button>
                  <button
                    class="py-1.5 px-3 rounded-lg text-xs font-medium cursor-pointer border-none bg-primary text-primary-content hover:opacity-90 transition-opacity disabled:opacity-40 disabled:cursor-not-allowed"
                    disabled={!editText.trim()}
                    onclick={() => saveEdit(origIdx)}
                  >Save & Submit</button>
                </div>
              </div>
            </div>
          </div>
        {:else}
          <div class="max-w-[640px] self-end mt-3">
            <div class="py-2.5 px-3.5 rounded-xl text-sm leading-relaxed bg-base-200 rounded-br-sm">
              {@html renderUserContent(msg.content)}
              {#if msg.attachments?.length}
                <div class="flex flex-wrap gap-2 mt-2">
                  {#each msg.attachments as att}
                    {@const attType = getAttachmentType(att.mimeType)}
                    {#if attType === 'image'}
                      <a href={att.url} target="_blank" rel="noopener" class="block">
                        <img
                          src={att.thumbnailUrl || att.url}
                          alt={att.filename}
                          class="max-w-[240px] max-h-[180px] rounded-lg border border-base-content/15 object-cover"
                          loading="lazy"
                        />
                      </a>
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
                title="Edit & resend"
                onclick={() => startEdit(origIdx, msg.content)}
              >
                <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>
              </button>
              <button
                class="w-7 h-7 rounded-md grid place-items-center {copiedIdx === origIdx ? 'text-success' : 'text-base-content/50 hover:text-base-content hover:bg-base-200'} cursor-pointer bg-transparent border-none transition-colors"
                title={copiedIdx === origIdx ? 'Copied!' : 'Copy'}
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
            Nebo worked for {msg.duration}
          </summary>
          <div class="mt-1.5 py-2 px-3 rounded-box bg-base-200 border-l-2 border-base-content/20 text-xs leading-relaxed font-mono whitespace-pre-wrap">{msg.content}</div>
        </details>

      {:else if msg.type === 'tool-group'}
        {@const hasRunning = msg.tools.some((t: ToolMsg) => t.status === 'running')}
        {@const isOpen = !!collapsedToolGroups[idx]}
        <div class="max-w-[640px] my-1">
          <button
            class="flex items-center gap-1.5 text-xs text-base-content/50 cursor-pointer bg-transparent border-none p-0 hover:text-base-content/70 transition-colors"
            onclick={() => toggleToolGroup(idx)}
          >
            {#if hasRunning}
              <svg width="14" height="14" viewBox="0 0 14 14" class="animate-spin text-primary shrink-0"><circle cx="7" cy="7" r="5.5" stroke="currentColor" stroke-width="1.5" fill="none" stroke-dasharray="20 14" stroke-linecap="round"/></svg>
              <span class="text-xs text-base-content/70">Running {msg.tools.length} tool{msg.tools.length !== 1 ? 's' : ''}...</span>
            {:else}
              <span class="text-xs">Used {msg.tools.length} tool{msg.tools.length !== 1 ? 's' : ''}</span>
              <span class="text-xs transition-transform {isOpen ? 'rotate-180' : ''}">&darr;</span>
            {/if}
          </button>

          {#if isOpen}
            <div class="mt-2 ml-1 flex flex-col">
              {#each msg.tools as tool, tidx}
                {@const resultKey = `${idx}-${tidx}`}
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
                    {#if tidx < msg.tools.length - 1 || isExpanded}
                      <div class="w-px flex-1 min-h-[28px] bg-base-300"></div>
                    {/if}
                  </div>
                  <div class="flex-1 min-w-0 pb-3">
                    <div class="text-xs font-mono {tool.status === 'running' ? 'text-base-content/70' : ''}">{tool.name}{#if tool.status === 'running'}<span class="text-base-content/50 ml-1">{tool.statusText || 'running...'}</span>{/if}</div>
                    {#if tool.status !== 'running'}
                      {#if isExpanded}
                        <div class="mt-2 rounded-lg border border-base-300 bg-base-100 overflow-hidden">
                          <div class="px-3.5 pt-3 pb-2">
                            <div class="text-xs font-semibold mb-1.5">Request</div>
                            <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap">{JSON.stringify(tool.request, null, 2)}</pre>
                          </div>
                          <div class="px-3.5 pt-2 pb-3 border-t border-base-300">
                            <div class="text-xs font-semibold mb-1.5">Response</div>
                            <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap">{tool.response}</pre>
                          </div>
                        </div>
                        <button
                          class="mt-1.5 py-0.5 px-2 rounded text-xs font-medium bg-base-200 cursor-pointer border-none hover:bg-base-300 transition-colors"
                          onclick={() => toggleResult(resultKey)}
                        >Hide</button>
                      {:else}
                        <div class="mt-1">
                          <button
                            class="py-0.5 px-2 rounded text-xs font-medium cursor-pointer border-none transition-colors {tool.status === 'success' ? 'bg-base-200 hover:bg-base-300' : 'bg-error/10 text-error hover:bg-error/20'}"
                            onclick={() => toggleResult(resultKey)}
                          >Result</button>
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
                  <span class="text-xs">Done</span>
                </div>
              {/if}
            </div>
          {/if}
        </div>

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
        <div class="max-w-[640px] mt-3">
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
          <div class="text-sm leading-relaxed prose prose-sm max-w-none">
            {#if msg.html}
              {@html renderMentionChips(msg.html)}
            {:else}
              {@html renderMarkdown(msg.content)}
            {/if}
          </div>
          {#if msg.attachments?.length}
            <div class="flex flex-wrap gap-2 mt-2">
              {#each msg.attachments as att}
                {@const attType = getAttachmentType(att.mimeType)}
                {#if attType === 'image'}
                  <a href={att.url} target="_blank" rel="noopener" class="block">
                    <img
                      src={att.thumbnailUrl || att.url}
                      alt={att.filename}
                      class="max-w-[240px] max-h-[180px] rounded-lg border border-base-content/15 object-cover"
                      loading="lazy"
                    />
                  </a>
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
                <div class="text-xs text-base-content/50">{artifact.kind === 'code' ? 'Code' : artifact.kind === 'table' ? 'Spreadsheet' : artifact.kind === 'slides' ? 'Presentation' : 'Document'}</div>
              </div>
            </button>
          {/each}

          <div class="flex items-center gap-1 mt-2">
            {#if msg.time}
              <span class="text-xs text-base-content/50 font-mono mr-1">{msg.time}</span>
            {/if}
            <button
              class="w-7 h-7 rounded-md grid place-items-center {copiedIdx === origIdx ? 'text-success' : 'text-base-content/50 hover:text-base-content hover:bg-base-200'} cursor-pointer bg-transparent border-none transition-colors"
              title={copiedIdx === origIdx ? 'Copied!' : 'Copy'}
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
              title="Retry"
              onclick={() => redoMessage(origIdx)}
            >
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
            </button>
          </div>
        </div>
      {/if}
    {/each}

    {#if isLoading && groupedMessages.length > 0 && groupedMessages[groupedMessages.length - 1]?.type !== 'assistant'}
      <div class="max-w-[640px] mt-3 py-2 flex items-center gap-2">
        <span class="loading loading-spinner loading-xs text-primary"></span>
        <span class="text-sm text-base-content/50 animate-pulse">{activityStatus || 'Working...'}</span>
      </div>
    {/if}
  </div>
  </div>
  </div>
  {/if}

  <!-- Token usage badge -->
  {#if tokenUsage}
    {@const uncachedInput = Math.max(0, tokenUsage.input - (tokenUsage.cacheRead ?? 0) - (tokenUsage.cacheCreation ?? 0))}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-6 pb-1">
      <span class="text-xs text-base-content/50 font-mono" title="{tokenUsage.input.toLocaleString()} total · {(tokenUsage.cacheRead ?? 0).toLocaleString()} cache read · {(tokenUsage.cacheCreation ?? 0).toLocaleString()} cache write">
        {uncachedInput.toLocaleString()} in · {tokenUsage.output.toLocaleString()} out
      </span>
    </div>
  {/if}

  <!-- Follow-up suggestion chips -->
  {#if followupSuggestions.length > 0 && !isLoading}
    <div class="max-w-3xl mx-auto w-full shrink-0 px-4 pb-2 flex flex-wrap items-center gap-2">
      {#each followupSuggestions as suggestion}
        <button
          class="btn btn-sm btn-outline btn-primary rounded-full text-xs"
          onclick={() => onfollowupselect?.(suggestion)}
        >
          {suggestion}
        </button>
      {/each}
      <button
        class="text-xs text-base-content/40 hover:text-base-content/70 transition-colors cursor-pointer bg-transparent border-none px-1"
        onclick={() => ondismissfollowups?.()}
      >dismiss</button>
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
      bind:this={composerRef}
    />
  </div>
</div>

<!-- Resize handle + Creations panel -->
{#if creationsOpen}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div
    class="w-0 shrink-0 cursor-col-resize relative z-10 group"
    onmousedown={startResize}
    role="separator"
    aria-orientation="vertical"
    tabindex="0"
  >
    <!-- Hover hit area (wider than visible) -->
    <div class="absolute inset-y-0 -left-2 -right-2"></div>
    <!-- Visible line — appears on hover with delay -->
    <div class="absolute inset-y-0 -left-px w-0.5 bg-primary/30 opacity-0 group-hover:opacity-100 transition-opacity duration-300 delay-150 {resizing ? '!opacity-100' : ''}"></div>
    <!-- Grip handle — centered dot pattern -->
    <div class="absolute top-1/2 -translate-y-1/2 -left-1.5 w-3 h-8 rounded-full bg-base-300 border border-base-content/10 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity duration-300 delay-150 {resizing ? '!opacity-100' : ''}">
      <div class="flex flex-col gap-0.5">
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
        <div class="w-0.5 h-0.5 rounded-full bg-base-content/30"></div>
      </div>
    </div>
  </div>
  <!-- Creations panel -->
  <div class="flex flex-col bg-base-100 min-h-0 min-w-0 overflow-hidden shrink-0 border-l border-base-300" style="width: {creationsWidth}px">
    <!-- Creations header -->
    <div class="h-11 px-4 border-b border-base-content/10 flex items-center gap-2 shrink-0">
      {#if activeArtifact}
        <!-- Artifact tabs -->
        <div class="flex items-center gap-1 flex-1 min-w-0 overflow-x-auto">
          {#each artifacts as a}
            {@const ArtIcon2 = artifactIcons[a.kind]}
            <button
              class="flex items-center gap-1.5 py-1 px-2 rounded-md text-xs cursor-pointer border-none transition-colors shrink-0 {activeArtifactId === a.id ? 'bg-base-200 font-medium text-base-content' : 'bg-transparent text-base-content/50 hover:text-base-content/70 hover:bg-base-200/50'}"
              onclick={() => openArtifact(a.id)}
            >
              {#if ArtIcon2}<ArtIcon2 class="w-3 h-3 shrink-0" />{/if}
              <span class="truncate max-w-[100px]">{a.title}</span>
            </button>
          {/each}
        </div>
      {:else}
        <span class="text-sm font-semibold flex-1 truncate">{creationsTitle}</span>
      {/if}
      <button
        class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/70 hover:text-base-content transition-colors shrink-0"
        onclick={() => creationsOpen = false}
        title="Close Work panel"
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
      </button>
    </div>
    <!-- Creations content -->
    <div class="flex-1 overflow-y-auto">
      {#if activeArtifact}
        <div class="p-4">
          {#if activeArtifact.kind === 'code'}
            <pre class="text-xs font-mono leading-relaxed whitespace-pre-wrap rounded-lg bg-base-200 p-4 overflow-x-auto">{activeArtifact.preview}</pre>
          {:else if activeArtifact.kind === 'table'}
            {@const lines = activeArtifact.preview.split('\n')}
            {@const headerCells = lines[0]?.split('|').map(c => c.trim()).filter(Boolean) ?? []}
            {@const bodyLines = lines.slice(1)}
            <div class="overflow-x-auto rounded-lg border border-base-300">
              <table class="table table-xs w-full">
                <thead>
                  <tr class="bg-base-200">
                    {#each headerCells as cell}
                      <th class="text-xs font-semibold">{cell}</th>
                    {/each}
                  </tr>
                </thead>
                <tbody>
                  {#each bodyLines as line}
                    <tr class="border-t border-base-300">
                      {#each line.split('|').map(c => c.trim()).filter(Boolean) as cell}
                        <td class="text-xs">{cell}</td>
                      {/each}
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {:else}
            <!-- Document / slides: render simple markdown -->
            <div class="text-sm leading-relaxed">
              {#each activeArtifact.preview.split('\n') as line}
                {#if line.startsWith('## ')}
                  <h2 class="text-base font-semibold mt-4 mb-2">{line.slice(3)}</h2>
                {:else if line.startsWith('### ')}
                  <h3 class="text-sm font-semibold mt-3 mb-1">{line.slice(4)}</h3>
                {:else if line.startsWith('**') && line.endsWith('**')}
                  <p class="font-semibold mt-2">{line.replace(/\*\*/g, '')}</p>
                {:else if line.startsWith('- ')}
                  <p class="ml-3">&bull; {line.slice(2)}</p>
                {:else if line.trim() === ''}
                  <div class="h-2"></div>
                {:else}
                  <p>{line}</p>
                {/if}
              {/each}
            </div>
          {/if}
        </div>
      {:else}
        <div class="flex flex-col items-center justify-center h-full gap-3 text-base-content/50 p-6">
          <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="3" width="18" height="18" rx="2"/>
            <path d="M9 3v18"/>
            <path d="M14 9l3 3-3 3"/>
          </svg>
          <div class="text-sm font-medium">Nothing here yet</div>
          <div class="text-xs text-center max-w-[220px]">When Nebo makes a report, sheet, or design, it'll appear here.</div>
        </div>
      {/if}
    </div>
  </div>
{/if}
</div>
