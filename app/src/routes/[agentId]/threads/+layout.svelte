<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { getContext } from 'svelte';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';
  import { deleteChat, updateChat } from '$lib/api/nebo';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const threads = $derived(ctx.threads);
  const isThreadsLoading = $derived(ctx.isThreadsLoading);
  const agentStatus = $derived(ctx.agentStatus(ctx.agentId));
  const selectedThread = $derived($page.params.threadId || '');

  function selectThread(id: string) {
    goto(`/${agentId}/threads/${id}`, { replaceState: true, keepFocus: true });
  }

  // Context menu state
  let ctxMenu = $state<{ threadId: string; x: number; y: number } | null>(null);
  let renaming = $state<{ threadId: string; value: string } | null>(null);
  let renameInput = $state<HTMLInputElement | null>(null);

  function openCtxMenu(e: MouseEvent, threadId: string) {
    e.preventDefault();
    e.stopPropagation();
    ctxMenu = { threadId, x: e.clientX, y: e.clientY };
  }

  function closeCtxMenu() {
    ctxMenu = null;
  }

  async function handleDelete(threadId: string) {
    closeCtxMenu();
    try {
      await deleteChat(threadId);
      await ctx.refreshThreads();
      if (selectedThread === threadId) {
        goto(`/${agentId}/threads`, { replaceState: true });
      }
    } catch (e) {
      console.error('[nebo] Failed to delete thread:', e);
    }
  }

  function startRename(threadId: string) {
    closeCtxMenu();
    const thread = threads.find(t => t.id === threadId);
    if (!thread) return;
    renaming = { threadId, value: thread.name };
    // Focus input on next tick
    setTimeout(() => renameInput?.focus(), 0);
  }

  async function commitRename() {
    if (!renaming) return;
    const { threadId, value } = renaming;
    const trimmed = value.trim();
    renaming = null;
    if (!trimmed) return;
    try {
      await updateChat(threadId, { title: trimmed });
      await ctx.refreshThreads();
    } catch (e) {
      console.error('[nebo] Failed to rename thread:', e);
    }
  }

  function cancelRename() {
    renaming = null;
  }

  function handleRenameKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      commitRename();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelRename();
    }
  }
</script>

<!-- Context menu backdrop -->
{#if ctxMenu}
  <div class="fixed inset-0 z-50" onclick={closeCtxMenu} oncontextmenu={(e) => { e.preventDefault(); closeCtxMenu(); }} role="presentation"></div>
  <div
    class="fixed z-50 w-[160px] py-1 rounded-lg border border-base-300 bg-base-100 shadow-xl"
    style="left: {ctxMenu.x}px; top: {ctxMenu.y}px;"
  >
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors" onclick={() => startRename(ctxMenu!.threadId)}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>
      Rename
    </button>
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors text-error" onclick={() => handleDelete(ctxMenu!.threadId)}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
      Delete
    </button>
  </div>
{/if}

<!-- Column 2: Thread list -->
<div class="w-[260px] min-w-[260px] border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatus} isApp={ctx.isApp} />

  <div class="flex-1 overflow-y-auto">
    <!-- New chat -->
    <a href="/{agentId}/threads" class="block w-full text-left py-2.5 px-3.5 border-b border-base-300 cursor-pointer hover:bg-base-200 transition-colors no-underline text-base-content">
      <div class="text-sm font-medium text-primary">+ New chat</div>
      <div class="text-xs text-base-content/70">Clean context. {agent?.name} knows who you are but nothing about previous chats.</div>
    </a>

    {#each threads as t}
      <a
        href="/{agentId}/threads/{t.id}"
        class="group relative block w-full text-left py-2.5 px-3.5 border-b border-base-300 cursor-pointer transition-colors no-underline text-base-content {selectedThread === t.id
          ? 'bg-base-100 border-l-2 border-l-primary'
          : 'bg-transparent border-l-2 border-l-transparent hover:bg-base-200'}"
        oncontextmenu={(e) => openCtxMenu(e, t.id)}
      >
        {#if renaming?.threadId === t.id}
          <input
            bind:this={renameInput}
            type="text"
            class="input input-xs input-bordered w-full text-sm font-medium mb-0.5"
            bind:value={renaming.value}
            onkeydown={handleRenameKeydown}
            onblur={commitRename}
            onclick={(e) => e.preventDefault()}
          />
        {:else}
          <div class="text-sm font-medium truncate mb-0.5">{t.name}</div>
        {/if}
        <div class="text-xs text-base-content/70 truncate mb-0.5">{t.preview}</div>
        <div class="text-xs text-base-content/50 font-mono">{t.messages} messages &middot; {t.updatedAt}</div>

        <!-- Three-dot menu button (visible on hover) -->
        <button
          class="absolute top-2 right-2 p-1 rounded opacity-0 group-hover:opacity-100 hover:bg-base-300 transition-opacity cursor-pointer bg-transparent border-none"
          onclick={(e) => { e.preventDefault(); e.stopPropagation(); openCtxMenu(e, t.id); }}
          aria-label="Thread options"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" class="text-base-content/70">
            <circle cx="12" cy="5" r="2"/>
            <circle cx="12" cy="12" r="2"/>
            <circle cx="12" cy="19" r="2"/>
          </svg>
        </button>
      </a>
    {/each}

    {#if isThreadsLoading && threads.length === 0}
      <div class="p-6 flex justify-center">
        <span class="loading loading-spinner loading-sm"></span>
      </div>
    {:else if threads.length === 0}
      <div class="p-6 text-center text-sm text-base-content/50">No chats yet. Start a new one.</div>
    {/if}
  </div>
</div>

<!-- Column 3: Chat content from child page -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  {@render children()}
</div>
