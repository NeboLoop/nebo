<script lang="ts">
  import { onMount } from 'svelte';
  import SlashCommandMenu from './SlashCommandMenu.svelte';
  import type { SlashCommand } from './slashCommands.js';
  import { AGENT_COLORS_MAP } from '$lib/tokens.js';

  interface AttachedFile {
    file: File;
    id: string;
    previewUrl: string | null;
    isImage: boolean;
  }

  let { agentName = 'Agent', agentId = '', placeholder = '', onsend, isLoading = false }: {
    agentName?: string;
    agentId?: string;
    placeholder?: string;
    onsend?: (text: string, files: AttachedFile[]) => void;
    isLoading?: boolean;
  } = $props();

  let allAgents = $state<{ id: string; name: string; role: string; initial: string; status: string; color: string }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgents();
      if (resp?.agents?.length) {
        allAgents = resp.agents.map(a => ({
          id: a.id,
          name: a.name,
          role: a.description || '',
          initial: a.name.charAt(0).toUpperCase(),
          status: a.isEnabled ? 'online' : 'paused',
          color: 'teal',
        }));
      }
    } catch { /* keep mock agents */ }
  });

  let text = $state('');
  let textareaEl = $state<HTMLTextAreaElement | null>(null);
  let fileInputEl = $state<HTMLInputElement | null>(null);
  let slashMenuRef = $state<{ handleKey: (e: KeyboardEvent) => boolean } | null>(null);
  let attachments = $state<AttachedFile[]>([]);

  // Composer-level drag state
  let composerDragOver = $state(false);
  let composerDragDepth = 0;

  const hasContent = $derived(text.trim().length > 0 || attachments.length > 0);

  // Slash command detection
  const showSlashMenu = $derived(text.startsWith('/') && !text.includes(' '));
  const slashQuery = $derived(showSlashMenu ? text : '');

  // @ mention detection
  let mentionMenuVisible = $state(false);
  let mentionQuery = $state('');
  let mentionActiveIdx = $state(0);

  function detectMention() {
    if (!textareaEl) return;
    const cursorPos = textareaEl.selectionStart;
    const beforeCursor = text.slice(0, cursorPos);
    const atIdx = beforeCursor.lastIndexOf('@');

    if (atIdx >= 0) {
      const charBefore = atIdx > 0 ? beforeCursor[atIdx - 1] : ' ';
      if (charBefore === ' ' || charBefore === '\n' || atIdx === 0) {
        const query = beforeCursor.slice(atIdx + 1);
        if (!query.includes(' ') && !query.includes('\n')) {
          mentionQuery = query;
          mentionMenuVisible = true;
          mentionActiveIdx = 0;
          return;
        }
      }
    }
    mentionMenuVisible = false;
    mentionQuery = '';
  }

  const mentionAgents = $derived(
    allAgents
      .filter(a => a.id !== agentId)
      .filter(a => !mentionQuery || a.name.toLowerCase().includes(mentionQuery.toLowerCase()))
  );

  function insertMention(agent: typeof allAgents[0]) {
    if (!textareaEl) return;
    const cursorPos = textareaEl.selectionStart;
    const beforeCursor = text.slice(0, cursorPos);
    const atIdx = beforeCursor.lastIndexOf('@');
    const afterCursor = text.slice(cursorPos);

    text = beforeCursor.slice(0, atIdx) + '@' + agent.name + ' ' + afterCursor;
    mentionMenuVisible = false;
    mentionQuery = '';

    requestAnimationFrame(() => {
      if (textareaEl) {
        const newPos = atIdx + agent.name.length + 2;
        textareaEl.selectionStart = newPos;
        textareaEl.selectionEnd = newPos;
        textareaEl.focus();
      }
    });
  }

  function scrollMentionIntoView() {
    requestAnimationFrame(() => {
      const el = document.querySelector('[data-mention-idx="' + mentionActiveIdx + '"]');
      if (el) el.scrollIntoView({ block: 'nearest' });
    });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (showSlashMenu && slashMenuRef?.handleKey(e)) return;

    if (mentionMenuVisible && mentionAgents.length > 0) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        mentionActiveIdx = (mentionActiveIdx + 1) % mentionAgents.length;
        scrollMentionIntoView();
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        mentionActiveIdx = (mentionActiveIdx - 1 + mentionAgents.length) % mentionAgents.length;
        scrollMentionIntoView();
        return;
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
        e.preventDefault();
        insertMention(mentionAgents[mentionActiveIdx]);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        mentionMenuVisible = false;
        return;
      }
    }

    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  function send() {
    if (!hasContent || isLoading) return;
    onsend?.(text.trim(), attachments);
    text = '';
    clearAttachments();
    mentionMenuVisible = false;
    if (textareaEl) textareaEl.style.height = 'auto';
  }

  function handleSlashSelect(cmd: SlashCommand) {
    if (cmd.args) {
      text = cmd.name + ' ';
    } else {
      onsend?.(cmd.name, []);
      text = '';
    }
    textareaEl?.focus();
  }

  function handleSlashClose() {
    text = '';
    textareaEl?.focus();
  }

  function handleInput() {
    if (textareaEl) {
      textareaEl.style.height = 'auto';
      textareaEl.style.height = Math.min(textareaEl.scrollHeight, 200) + 'px';
    }
    detectMention();
  }

  function browseFiles() {
    if (fileInputEl) fileInputEl.click();
  }

  function handleFileInput(e: Event) {
    const target = e.target as HTMLInputElement;
    const files = Array.from(target.files || []);
    if (files.length) addFiles(files);
    if (fileInputEl) fileInputEl.value = '';
  }

  // File attachment management
  export function addFiles(files: File[]) {
    for (const file of files) {
      const isImage = file.type.startsWith('image/');
      const previewUrl = isImage ? URL.createObjectURL(file) : null;
      attachments.push({
        file,
        id: crypto.randomUUID(),
        previewUrl,
        isImage
      });
    }
    textareaEl?.focus();
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

  // Composer drag handlers
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
    // Don't stopPropagation — ChatPane needs the event to clear its overlay
    (e as Event & { _composerHandled?: boolean })._composerHandled = true;
    composerDragOver = false;
    composerDragDepth = 0;
    const files = Array.from(e.dataTransfer?.files || []);
    if (files.length) addFiles(files);
  }

  export function focus() {
    textareaEl?.focus();
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
            onmousedown={(e) => { e.preventDefault(); insertMention(agent); }}
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
            <!-- Image thumbnail -->
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
            <!-- File chip -->
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

    <textarea
      bind:this={textareaEl}
      bind:value={text}
      rows="1"
      placeholder={placeholder || `Message ${agentName}...`}
      class="w-full text-base outline-none resize-none bg-transparent leading-snug"
      onkeydown={handleKeydown}
      oninput={handleInput}
    ></textarea>

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
      </div>

      {#if isLoading}
        <button
          class="btn btn-error btn-circle size-9 text-sm"
          title="Stop"
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
