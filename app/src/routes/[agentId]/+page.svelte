<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { getContext } from 'svelte';
  import type { AgentPageContext } from '$lib/types/agentPage';

  const ctx = getContext<AgentPageContext>('agentPage');

  $effect(() => {
    const id = $page.params.agentId;
    if (!id) return;
    // Wait for roster to load so we know if agent is an app
    if (ctx.agentsLoading) return;
    if (ctx.agent?.isApp) {
      goto(`/${id}/overview`, { replaceState: true });
    } else {
      goto(`/${id}/threads`, { replaceState: true });
    }
  });
</script>
