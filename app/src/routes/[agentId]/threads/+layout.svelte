<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import { t } from 'svelte-i18n';
  import { getContext } from 'svelte';
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
      {$t('sidebar.rename')}
    </button>
    <button class="flex items-center gap-2.5 w-full px-3 py-1.5 text-sm text-left cursor-pointer bg-transparent border-none hover:bg-base-200 transition-colors text-error" onclick={() => handleDelete(ctxMenu!.threadId)}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/></svg>
      {$t('common.delete')}
    </button>
  </div>
{/if}

<!-- The chat list now lives in the workspace column, grouped under its
     employee. This layout is just the conversation. -->
<!-- Column 3: Chat content from child page -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  <!-- SvelteKit reuses the page component across conversation switches (the
       route id never changes), but the page captures agentId and creates its
       chat controller ONCE — the controller filters every WS event through
       that frozen agent id, so switching employees left the transcript stuck
       on the previous one. The page was designed around remounts (its
       pending-send stash exists to survive them); this key gives it real
       ones: a different conversation is a different page instance.
       Search-param changes (?settings, ?pane, ?run) don't touch the key, so
       drafts still survive modals. -->
  {#key `${$page.params.agentId}:${$page.params.threadId ?? ''}`}
    {@render children()}
  {/key}
</div>
