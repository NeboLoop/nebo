<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Editor, Extension, Mark } from '@tiptap/core';
  import StarterKit from '@tiptap/starter-kit';
  import Mention from '@tiptap/extension-mention';
  import SlashCommandMenu from './SlashCommandMenu.svelte';
  import VoiceButton from './VoiceButton.svelte';
  import VoiceModeOverlay from './VoiceModeOverlay.svelte';
  import type { SlashCommand } from './slashCommands.js';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';
  import { dictationStore, combinedTranscript } from '$lib/stores/dictation';
  import AudioLines from 'lucide-svelte/icons/audio-lines';

  interface AttachedFile {
    file: File;
    id: string;
    previewUrl: string | null;
    isImage: boolean;
  }

  interface MentionRef {
    id: string;
    name: string;
  }

  type AgentInfo = { id: string; name: string; role: string; initial: string; status: string; color: string };

  let { agentName = 'Agent', agentId = '', placeholder = '', allAgents = [], onsend, onstop, isLoading = false }: {
    agentName?: string;
    agentId?: string;
    placeholder?: string;
    allAgents?: AgentInfo[];
    onsend?: (text: string, files: AttachedFile[], mentions?: MentionRef[]) => void;
    onstop?: () => void;
    isLoading?: boolean;
  } = $props();

  let editorElement = $state<HTMLDivElement | null>(null);
  let fileInputEl = $state<HTMLInputElement | null>(null);
  let slashMenuRef = $state<{ handleKey: (e: KeyboardEvent) => boolean } | null>(null);
  let attachments = $state<AttachedFile[]>([]);
  let editor: Editor | null = null;

  // Track emptiness reactively (TipTap doesn't trigger Svelte reactivity)
  let editorIsEmpty = $state(true);

  // Slash command state
  let showSlashMenu = $state(false);
  let slashQuery = $state('');

  // Mention popup state (driven by TipTap suggestion)
  let mentionMenuVisible = $state(false);
  let mentionQuery = $state('');
  let mentionActiveIdx = $state(0);
  let mentionCommand: ((props: { id: string; label: string }) => void) | null = null;

  // Dictation — unique owner ID for this composer instance
  const composerOwnerId = crypto.randomUUID();
  let isDictating = $derived($dictationStore.status === 'recording' && $dictationStore.ownerId === composerOwnerId);

  // Dictation doc builder state — frozen cursor segments (Phase 7.6)
  let dictationBefore = $state('');
  let dictationAfter = $state('');

  // Voice conversation overlay state
  let showVoiceOverlay = $state(false);

  // IME composition state (Phase 10 — prevents Enter-to-send during CJK input)
  let isComposing = $state(false);

  // Draft persistence (Phase 6)
  let draftSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let hasHydrated = $state(false);
  const draftKey = $derived(agentId ? `nebo:draft:${agentId}` : '');

  function saveDraft() {
    if (!editor || !draftKey) return;
    if (editor.isEmpty) {
      localStorage.removeItem(draftKey);
    } else {
      localStorage.setItem(draftKey, JSON.stringify(editor.getJSON()));
    }
  }

  function debouncedSaveDraft() {
    if (draftSaveTimer) clearTimeout(draftSaveTimer);
    draftSaveTimer = setTimeout(saveDraft, 300);
  }

  function restoreDraft() {
    if (!editor || !draftKey) return;
    try {
      const saved = localStorage.getItem(draftKey);
      if (saved) {
        const json = JSON.parse(saved);
        editor.commands.setContent(json);
        editorIsEmpty = editor.isEmpty;
      }
    } catch { /* ignore corrupt drafts */ }
    hasHydrated = true;
  }

  function clearDraft() {
    if (draftKey) localStorage.removeItem(draftKey);
    if (draftSaveTimer) clearTimeout(draftSaveTimer);
  }

  // Computed
  const hasContent = $derived(!editorIsEmpty || attachments.length > 0);

  const mentionAgents = $derived(
    allAgents
      .filter(a => a.id !== agentId)
      .filter(a => !mentionQuery || a.name.toLowerCase().includes(mentionQuery.toLowerCase()))
  );

  // Composer-level drag state
  let composerDragOver = $state(false);
  let composerDragDepth = 0;

  // --- Custom Dictation Mark (uses Tailwind classes in renderHTML) ---
  const DictationMark = Mark.create({
    name: 'dictation',
    parseHTML() {
      return [{ tag: 'span[data-dictation]' }];
    },
    renderHTML() {
      return ['span', { 'data-dictation': '', class: 'bg-primary/20 border-b-2 border-primary/60 rounded-sm' }, 0];
    },
  });

  // --- Dictation doc builder (Phase 7.6 — matches Claude Desktop KVt pattern) ---

  function buildDictationDoc(before: string, dictationText: string, after: string) {
    const fullText = before + dictationText + after;
    const dictStart = before.length;
    const dictEnd = before.length + dictationText.length;
    const lines = fullText.split('\n');
    let offset = 0;

    return {
      type: 'doc' as const,
      content: lines.map(line => {
        const lineStart = offset;
        const lineEnd = offset + line.length;
        offset = lineEnd + 1;

        if (line.length === 0) return { type: 'paragraph' as const };

        const segments: Array<{ text: string; isDictation: boolean }> = [];

        if (lineStart < dictStart && lineStart < lineEnd) {
          const end = Math.min(dictStart, lineEnd);
          const text = line.slice(0, end - lineStart);
          if (text) segments.push({ text, isDictation: false });
        }

        if (lineEnd > dictStart && lineStart < dictEnd) {
          const start = Math.max(dictStart, lineStart);
          const end = Math.min(dictEnd, lineEnd);
          const text = line.slice(start - lineStart, end - lineStart);
          if (text) segments.push({ text, isDictation: true });
        }

        if (lineEnd > dictEnd) {
          const start = Math.max(dictEnd, lineStart);
          const text = line.slice(start - lineStart);
          if (text) segments.push({ text, isDictation: false });
        }

        const content = segments.map(seg => {
          const node: Record<string, unknown> = { type: 'text', text: seg.text };
          if (seg.isDictation) {
            node.marks = [{ type: 'dictation' }];
          }
          return node;
        });

        return {
          type: 'paragraph' as const,
          content: content.length > 0 ? content : undefined
        };
      })
    };
  }

  function textOffsetToDocPos(fullText: string, textOffset: number, docContentSize: number): number {
    let extra = 0;
    for (let i = 0; i < textOffset && i < fullText.length; i++) {
      if (fullText[i] === '\n') extra++;
    }
    return Math.min(textOffset + 1 + extra, docContentSize);
  }

  // --- Cmd+D hotkey for dictation toggle (Phase 7.7) ---
  function handleDictationHotkey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 'd') {
      e.preventDefault();
      if (isDictating) {
        dictationStore.stop();
      } else {
        dictationStore.start(composerOwnerId, { type: 'editor' });
      }
    }
  }

  // --- Slash Command Detection Extension ---
  const SlashDetector = Extension.create({
    name: 'slashDetector',
    onUpdate({ editor: ed }) {
      const text = ed.getText();
      if (text.startsWith('/') && !text.includes(' ')) {
        showSlashMenu = true;
        slashQuery = text;
      } else {
        showSlashMenu = false;
        slashQuery = '';
      }
    },
  });

  // --- Reactive dictation integration (Phase 7.6) ---

  // Track dictation start/stop transitions
  let wasDictatingRef = false;

  $effect(() => {
    const active = isDictating;

    if (active && !wasDictatingRef) {
      // Dictation just started — capture cursor position (frozen segments)
      if (editor) {
        const { from } = editor.state.selection;
        const fullText = editor.state.doc.textBetween(0, editor.state.doc.content.size, '\n');
        const beforeText = editor.state.doc.textBetween(0, from, '\n');
        dictationBefore = beforeText;
        dictationAfter = fullText.slice(beforeText.length);
      }
    }

    if (!active && wasDictatingRef) {
      // Dictation just stopped — strip marks, position cursor
      if (editor) {
        const pos = editor.state.selection.anchor;
        editor.chain().selectAll().unsetMark('dictation').setTextSelection(pos).run();
      }
    }

    wasDictatingRef = active;
  });

  // Rebuild TipTap doc on each transcript update during dictation
  $effect(() => {
    const transcript = $combinedTranscript;
    if (!isDictating || !editor || !transcript) return;

    const needsLeading = dictationBefore.length > 0
      && !dictationBefore.endsWith(' ') && !dictationBefore.endsWith('\n')
      && !transcript.startsWith(' ');
    const needsTrailing = dictationAfter.length > 0
      && !dictationAfter.startsWith(' ') && !dictationAfter.startsWith('\n')
      && !transcript.endsWith(' ');
    const dictPart = (needsLeading ? ' ' : '') + transcript + (needsTrailing ? ' ' : '');

    const doc = buildDictationDoc(dictationBefore, dictPart, dictationAfter);
    editor.commands.setContent(doc);

    const fullText = dictationBefore + dictPart + dictationAfter;
    const cursorOffset = dictationBefore.length + dictPart.length;
    const cursorPos = textOffsetToDocPos(fullText, cursorOffset, editor.state.doc.content.size);
    editor.commands.setTextSelection(cursorPos);
  });

  // --- Initialize TipTap Editor ---
  onMount(() => {
    if (!editorElement) return;

    // Stop dictation when tab becomes hidden
    document.addEventListener('visibilitychange', dictationStore.handleVisibilityChange);

    // Cmd+D hotkey
    document.addEventListener('keydown', handleDictationHotkey);

    editor = new Editor({
      element: editorElement,
      extensions: [
        StarterKit.configure({
          heading: false,
          codeBlock: false,
          horizontalRule: false,
          blockquote: false,
        }),
        DictationMark,
        SlashDetector,
        Mention.configure({
          HTMLAttributes: { class: 'mention-chip' },
          suggestion: {
            char: '@',
            items: ({ query }) => {
              return allAgents
                .filter(a => a.id !== agentId)
                .filter(a => !query || a.name.toLowerCase().includes(query.toLowerCase()))
                .slice(0, 8);
            },
            render: () => ({
              onStart: (props: any) => {
                mentionMenuVisible = true;
                mentionQuery = props.query;
                mentionActiveIdx = 0;
                mentionCommand = props.command;
              },
              onUpdate: (props: any) => {
                mentionQuery = props.query;
                mentionActiveIdx = 0;
                mentionCommand = props.command;
              },
              onExit: () => {
                mentionMenuVisible = false;
                mentionQuery = '';
                mentionCommand = null;
              },
              onKeyDown: ({ event }: { event: KeyboardEvent }) => {
                if (!mentionMenuVisible || mentionAgents.length === 0) return false;
                if (event.key === 'ArrowDown') {
                  event.preventDefault();
                  mentionActiveIdx = (mentionActiveIdx + 1) % mentionAgents.length;
                  scrollMentionIntoView();
                  return true;
                }
                if (event.key === 'ArrowUp') {
                  event.preventDefault();
                  mentionActiveIdx = (mentionActiveIdx - 1 + mentionAgents.length) % mentionAgents.length;
                  scrollMentionIntoView();
                  return true;
                }
                if (event.key === 'Enter' || event.key === 'Tab') {
                  event.preventDefault();
                  selectMention(mentionAgents[mentionActiveIdx]);
                  return true;
                }
                if (event.key === 'Escape') {
                  event.preventDefault();
                  mentionMenuVisible = false;
                  return true;
                }
                return false;
              },
            }),
          },
          renderText({ node }: any) {
            return `@${node.attrs.label}`;
          },
          renderHTML({ node }: any) {
            const agent = allAgents.find(a => a.id === node.attrs.id);
            const c = AGENT_COLORS_MAP[agent?.color || 'teal'] || AGENT_COLORS_MAP['teal'];
            return ['span', {
              class: `inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-xs font-medium align-baseline mx-0.5 ${c.bgClass} ${c.inkClass}`,
              'data-mention-id': node.attrs.id,
              'data-mention-name': node.attrs.label,
            }, `@${node.attrs.label}`];
          },
        }),
      ],
      editorProps: {
        attributes: {
          class: 'w-full text-base outline-none bg-transparent leading-snug min-h-[1.5em] max-h-[200px] overflow-y-auto whitespace-pre-wrap break-words',
        },
        handleKeyDown(_view, event) {
          if (showSlashMenu && slashMenuRef?.handleKey(event)) return true;

          if (event.key === 'Escape' && isLoading) {
            event.preventDefault();
            onstop?.();
            return true;
          }

          // Phase 10: Suppress Enter during IME composition (CJK input)
          if (event.key === 'Enter' && !event.shiftKey && !mentionMenuVisible && !isComposing) {
            event.preventDefault();
            send();
            return true;
          }

          return false;
        },
        handlePaste(_view, event) {
          const files = Array.from(event.clipboardData?.files || []);
          if (files.length > 0) {
            event.preventDefault();
            addFiles(files);
            return true;
          }
          return false;
        },
      },
      onUpdate({ editor: ed }) {
        editorIsEmpty = ed.isEmpty;
        debouncedSaveDraft();
      },
      onCreate({ editor: ed }) {
        editorIsEmpty = ed.isEmpty;
        restoreDraft();
      },
    });
  });

  onDestroy(() => {
    saveDraft(); // Flush any pending draft before teardown
    if (draftSaveTimer) clearTimeout(draftSaveTimer);
    // Stop dictation if this composer owns the session (prevents orphaned sessions on navigation)
    if (dictationStore.isOwner(composerOwnerId)) {
      dictationStore.stop();
    }
    document.removeEventListener('visibilitychange', dictationStore.handleVisibilityChange);
    document.removeEventListener('keydown', handleDictationHotkey);
    editor?.destroy();
    editor = null;
  });

  // --- Mention helpers ---
  function selectMention(agent: AgentInfo) {
    if (mentionCommand) {
      mentionCommand({ id: agent.id, label: agent.name });
    }
    mentionMenuVisible = false;
  }

  function scrollMentionIntoView() {
    requestAnimationFrame(() => {
      const el = document.querySelector('[data-mention-idx="' + mentionActiveIdx + '"]');
      if (el) el.scrollIntoView({ block: 'nearest' });
    });
  }

  // --- Content serialization ---
  function serializeContent(): { text: string; mentions: MentionRef[] } {
    if (!editor) return { text: '', mentions: [] };
    const mentions: MentionRef[] = [];
    const json = editor.getJSON();

    function walkNode(node: any): string {
      if (node.type === 'text') return node.text || '';
      if (node.type === 'mention') {
        mentions.push({ id: node.attrs.id, name: node.attrs.label });
        return `<@${node.attrs.id}>`;
      }
      if (node.type === 'hardBreak') return '\n';
      if (node.type === 'paragraph') {
        return (node.content || []).map(walkNode).join('');
      }
      if (node.content) {
        return node.content.map(walkNode).join('\n');
      }
      return '';
    }

    const text = walkNode(json).trim();
    return { text, mentions };
  }

  // --- Send ---
  function send() {
    if (!hasContent || isLoading) return;
    const { text, mentions } = serializeContent();
    if (text || attachments.length > 0) {
      onsend?.(text, attachments, mentions);
    }
    editor?.commands.clearContent();
    editorIsEmpty = true;
    showSlashMenu = false;
    slashQuery = '';
    clearAttachments();
    clearDraft();
  }

  // --- Slash command handlers ---
  function handleSlashSelect(cmd: SlashCommand) {
    if (cmd.args) {
      editor?.commands.setContent(cmd.name + ' ');
      editor?.commands.focus('end');
      showSlashMenu = false;
    } else {
      onsend?.(cmd.name, []);
      editor?.commands.clearContent();
      editorIsEmpty = true;
      showSlashMenu = false;
      slashQuery = '';
      clearDraft();
    }
  }

  function handleSlashClose() {
    editor?.commands.clearContent();
    editorIsEmpty = true;
    showSlashMenu = false;
    slashQuery = '';
    editor?.commands.focus();
  }

  // --- Voice conversation ---
  function handleStartConversation() {
    showVoiceOverlay = true;
  }

  function handleCloseConversation() {
    showVoiceOverlay = false;
  }

  // --- File management ---
  function browseFiles() {
    if (fileInputEl) fileInputEl.click();
  }

  function handleFileInput(e: Event) {
    const target = e.target as HTMLInputElement;
    const files = Array.from(target.files || []);
    if (files.length) addFiles(files);
    if (fileInputEl) fileInputEl.value = '';
  }

  export function addFiles(files: File[]) {
    for (const file of files) {
      const isImage = file.type.startsWith('image/');
      const previewUrl = isImage ? URL.createObjectURL(file) : null;
      attachments.push({ file, id: crypto.randomUUID(), previewUrl, isImage });
    }
    editor?.commands.focus();
  }

  function removeAttachment(id: string) {
    const idx = attachments.findIndex(a => a.id === id);
    if (idx >= 0) {
      const att = attachments[idx];
      if (att.previewUrl) URL.revokeObjectURL(att.previewUrl);
      attachments.splice(idx, 1);
    }
  }

  function clearAttachments() {
    for (const att of attachments) {
      if (att.previewUrl) URL.revokeObjectURL(att.previewUrl);
    }
    attachments = [];
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
    return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
  }

  // --- Drag & Drop ---
  function onComposerDragEnter(e: DragEvent) {
    if (!Array.from(e.dataTransfer?.types ?? []).includes('Files')) return;
    e.preventDefault();
    composerDragDepth++;
    composerDragOver = true;
  }

  function onComposerDragOver(e: DragEvent) {
    if (!Array.from(e.dataTransfer?.types ?? []).includes('Files')) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
  }

  function onComposerDragLeave() {
    composerDragDepth = Math.max(0, composerDragDepth - 1);
    if (composerDragDepth === 0) composerDragOver = false;
  }

  function onComposerDrop(e: DragEvent) {
    e.preventDefault();
    (e as Event & { _composerHandled?: boolean })._composerHandled = true;
    composerDragOver = false;
    composerDragDepth = 0;
    const files = Array.from(e.dataTransfer?.files || []);
    if (files.length) addFiles(files);
  }

  export function focus() {
    editor?.commands.focus();
  }
</script>

<div class="px-6 py-3 shrink-0">
  <div
    class="rounded-box border shadow-md p-3 relative bg-surface transition-colors {composerDragOver ? 'border-primary ring-2 ring-primary/30' : 'border-base-300'}"
    ondragenter={onComposerDragEnter}
    ondragover={onComposerDragOver}
    ondragleave={onComposerDragLeave}
    ondrop={onComposerDrop}
  >
    {#if showSlashMenu}
      <SlashCommandMenu
        query={slashQuery}
        onselect={handleSlashSelect}
        onclose={handleSlashClose}
        bind:this={slashMenuRef}
      />
    {/if}

    {#if mentionMenuVisible && mentionAgents.length > 0}
      <div class="absolute bottom-full left-0 right-0 mb-2 z-20 bg-base-100 border border-base-300 rounded-xl shadow-lg max-h-[240px] overflow-y-auto">
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-4 pt-3 pb-1">Agents</div>
        {#each mentionAgents as agent, idx}
          {@const c = AGENT_COLORS_MAP[agent.color]}
          <button
            data-mention-idx={idx}
            class="flex items-center gap-2.5 px-4 py-2 w-full text-left cursor-pointer transition-colors border-none {idx === mentionActiveIdx ? 'bg-base-200' : 'bg-transparent hover:bg-base-200'}"
            onmouseenter={() => mentionActiveIdx = idx}
            onmousedown={(e) => { e.preventDefault(); selectMention(agent); }}
          >
            <div class="w-6 h-6 rounded-md flex items-center justify-center font-mono text-xs font-semibold shrink-0 {c.bgClass} {c.inkClass}">{agent.initial}</div>
            <div class="flex-1 min-w-0">
              <span class="text-sm font-medium">{agent.name}</span>
              <span class="text-xs text-base-content/70 ml-1.5">{agent.role}</span>
            </div>
          </button>
        {/each}
      </div>
    {/if}

    <!-- Attachments -->
    {#if attachments.length > 0}
      <div class="flex flex-wrap gap-2 mb-2">
        {#each attachments as att (att.id)}
          {#if att.isImage && att.previewUrl}
            <div class="relative group">
              <img
                src={att.previewUrl}
                alt={att.file.name}
                class="h-16 w-16 rounded-md object-cover border border-base-300"
              />
              <button
                class="absolute -top-1.5 -right-1.5 w-5 h-5 rounded-full bg-base-300 hover:bg-error hover:text-error-content flex items-center justify-center text-xs cursor-pointer border-none opacity-0 group-hover:opacity-100 transition-opacity"
                onclick={() => removeAttachment(att.id)}
                title="Remove"
              >&times;</button>
              <div class="absolute bottom-0 left-0 right-0 bg-base-content/60 text-base-100 text-xs px-1 py-0.5 rounded-b-md truncate">
                {att.file.name}
              </div>
            </div>
          {:else}
            <div class="flex items-center gap-1.5 py-1 pl-2 pr-1 rounded-md border border-base-300 bg-base-200/50 group">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="shrink-0 text-base-content/60">
                <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/>
              </svg>
              <span class="text-xs font-medium truncate max-w-[120px]">{att.file.name}</span>
              <span class="text-xs text-base-content/50 font-mono shrink-0">{formatSize(att.file.size)}</span>
              <button
                class="w-5 h-5 rounded-full hover:bg-error/20 hover:text-error flex items-center justify-center text-xs cursor-pointer border-none bg-transparent text-base-content/50 shrink-0 transition-colors"
                onclick={() => removeAttachment(att.id)}
                title="Remove"
              >&times;</button>
            </div>
          {/if}
        {/each}
      </div>
    {/if}

    <!-- TipTap Editor with placeholder overlay -->
    <div class="relative">
      {#if editorIsEmpty && hasHydrated}
        <div class="absolute inset-0 pointer-events-none text-base text-base-content/40 leading-snug">
          {placeholder || `Message ${agentName}...`}
        </div>
      {/if}
      <div
        bind:this={editorElement}
        oncompositionstart={() => isComposing = true}
        oncompositionend={() => isComposing = false}
      ></div>
    </div>

    <!-- Toolbar -->
    <div class="flex items-center justify-between mt-2">
      <div class="flex items-center gap-1">
        <button
          class="w-8 h-8 rounded-lg grid place-items-center text-base-content/60 hover:text-base-content hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent"
          onclick={browseFiles}
          title="Attach files"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48"/>
          </svg>
        </button>
        <VoiceButton
          ownerId={composerOwnerId}
        />
        <button
          class="w-8 h-8 rounded-lg grid place-items-center text-base-content/60 hover:text-base-content hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent"
          title="Voice conversation"
          onclick={handleStartConversation}
        >
          <AudioLines class="w-[18px] h-[18px]" />
        </button>
      </div>

      {#if isLoading}
        <button
          class="btn btn-error btn-circle size-9 text-sm"
          title="Stop (Esc)"
          onclick={onstop}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="1"/></svg>
        </button>
      {:else}
        <button
          class="btn btn-neutral btn-circle size-9 text-sm"
          disabled={!hasContent}
          onclick={send}
        >&#8593;</button>
      {/if}
    </div>
  </div>

  <input bind:this={fileInputEl} type="file" multiple class="hidden" onchange={handleFileInput} />

  <p class="text-center text-xs text-base-content/50 mt-2">
    Nebo can make mistakes. Verify important information.
  </p>
</div>

{#if showVoiceOverlay}
  <VoiceModeOverlay
    {agentId}
    agentName={agentName}
    onclose={handleCloseConversation}
  />
{/if}
