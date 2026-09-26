<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';
  import { getContext } from 'svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  let { children } = $props();

  const ctx = getContext<AgentPageContext>('agentPage');
  const agentId = $derived(ctx.agentId);
  const agent = $derived(ctx.agent);
  const isThreadsLoading = $derived(ctx.isThreadsLoading);
  const agentStatus = $derived(ctx.agentStatus(ctx.agentId));

  function selectThread(id: string) {
    goto(`/${agentId}/threads/${id}`, { replaceState: true, keepFocus: true });
  }
</script>

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
