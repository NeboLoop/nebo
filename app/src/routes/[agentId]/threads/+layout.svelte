<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { getContext } from 'svelte';
  import AgentTabBar from '$lib/components/AgentTabBar.svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const threads = $derived(ctx.threads);
  const isThreadsLoading = $derived(ctx.isThreadsLoading);
  const agentStatus = $derived(ctx.agentStatus(ctx.agentId));
  const selectedThread = $derived($page.params.threadId || '');

  let chatPaneRef = $state<{ focusComposer: () => void } | null>(null);

  // Auto-focus chat input when user starts typing
  function handleGlobalKeydown(e: KeyboardEvent) {
    if (document.activeElement?.tagName === 'INPUT' || document.activeElement?.tagName === 'TEXTAREA') return;
    if (e.ctrlKey || e.metaKey || e.altKey || e.key.length > 1) return;
    if (document.querySelector('[data-modal-open]')) return;
    chatPaneRef?.focusComposer();
  }

  function selectThread(id: string) {
    goto(`/${agentId}/threads/${id}`, { replaceState: true, keepFocus: true });
  }
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

<!-- Column 2: Thread list -->
<div class="w-[260px] min-w-[260px] border-r border-base-content/10 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.06)] relative shrink-0 flex flex-col bg-base-200/50">
  <AgentTabBar agentId={agentId} agentName={agent?.name ?? ''} agentInitial={agent?.initial ?? ''} status={agentStatus} />

  <div class="flex-1 overflow-y-auto">
    <!-- New thread -->
    <a href="/{agentId}/threads" class="block w-full text-left py-2.5 px-3.5 border-b border-base-300 cursor-pointer hover:bg-base-200 transition-colors no-underline text-base-content">
      <div class="text-sm font-medium text-primary">+ New Thread</div>
      <div class="text-xs text-base-content/70">Clean context. {agent?.name} knows who you are but nothing about previous threads.</div>
    </a>

    {#each threads as t}
      <a
        href="/{agentId}/threads/{t.id}"
        class="block w-full text-left py-2.5 px-3.5 border-b border-base-300 cursor-pointer transition-colors no-underline text-base-content {selectedThread === t.id
          ? 'bg-base-100 border-l-2 border-l-primary'
          : 'bg-transparent border-l-2 border-l-transparent hover:bg-base-200'}"
      >
        <div class="text-sm font-medium truncate mb-0.5">{t.name}</div>
        <div class="text-xs text-base-content/70 truncate mb-0.5">{t.preview}</div>
        <div class="text-xs text-base-content/50 font-mono">{t.messages} messages &middot; {t.updatedAt}</div>
      </a>
    {/each}

    {#if isThreadsLoading && threads.length === 0}
      <div class="p-6 flex justify-center">
        <span class="loading loading-spinner loading-sm"></span>
      </div>
    {:else if threads.length === 0}
      <div class="p-6 text-center text-sm text-base-content/50">No threads yet. Start a new one.</div>
    {/if}
  </div>
</div>

<!-- Column 3: Chat content from child page -->
<div class="flex-1 flex flex-col bg-base-100 min-w-0 min-h-0">
  {@render children()}
</div>
