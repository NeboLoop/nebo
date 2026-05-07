<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import type { Session } from '$lib/api/nebo';

  let sessions = $state<{ id: string; agent: string; messages: number; duration: string; time: string }[]>([]);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = await api.listAgentSessions();
      if (resp?.sessions?.length) {
        sessions = resp.sessions.map((s: Session) => ({
          id: s.id,
          agent: s.name || 'Agent',
          messages: s.messageCount ?? 0,
          duration: s.tokenCount ? `${s.tokenCount} tokens` : '',
          time: s.createdAt ? new Date(s.createdAt * 1000).toLocaleDateString() : '',
        }));
      }
    } catch { /* keep mock data */ }
  });
</script>

<svelte:head><title>Activity - Nebo</title></svelte:head>

<div class="flex h-screen bg-base-100 text-base-content text-sm">
  <Sidebar activePage="chat" />
  <div class="flex-1 flex flex-col min-w-0 min-h-0">
    <div class="h-12 px-5 border-b border-base-content/10 flex items-center gap-3.5 shrink-0">
      <span class="text-sm font-semibold">Activity</span>
      <div class="ml-auto h-7 w-[200px] rounded-md border border-base-content/10 bg-base-100 flex items-center px-2.5 gap-2 text-sm">
        <span class="font-mono">⌘K</span><span>Search or run…</span>
      </div>
    </div>

    <div class="flex-1 overflow-auto p-6">
      <div class="max-w-[800px]">
        <h1 class="text-xl font-bold tracking-tight mb-4">Session History</h1>

        <div class="flex flex-col gap-1.5">
          {#each sessions as session}
            <div class="flex items-center gap-3 py-2.5 px-3.5 rounded-lg border border-base-content/5 bg-base-100 cursor-pointer hover:border-base-content/15 transition-colors">
              <span class="text-sm font-medium w-[100px]">{session.agent}</span>
              <span class="font-mono text-xs w-[80px]">{session.messages} msgs</span>
              <span class="font-mono text-xs w-[60px]">{session.duration}</span>
              <span class="font-mono text-xs ml-auto">{session.time}</span>
            </div>
          {/each}
        </div>
      </div>
    </div>
  </div>
</div>
