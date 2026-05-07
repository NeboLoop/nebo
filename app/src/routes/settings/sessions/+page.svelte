<script lang="ts">
  import { onMount } from 'svelte';
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

<div class="mb-7">
  <h2 class="text-lg font-bold mb-1">Sessions</h2>
  <p class="text-xs text-base-content/70">View and manage conversation history.</p>
</div>

<!-- Stats -->
<div class="flex gap-2.5 mb-5">
  <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{sessions.length}</span> sessions</div>
  <div class="px-3.5 py-2 rounded-lg bg-base-200/50 text-sm"><span class="font-mono font-bold">{sessions.reduce((sum, s) => sum + s.messages, 0)}</span> messages</div>
</div>

<!-- Session list -->
<div class="flex flex-col gap-1.5 mb-6">
  {#each sessions as session}
    <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100 cursor-pointer hover:border-base-content/15 transition-colors">
      <div class="w-8 h-8 rounded-lg bg-base-200 grid place-items-center text-sm font-semibold shrink-0">{session.agent.charAt(0)}</div>
      <div class="flex-1">
        <div class="text-sm font-medium">{session.agent}</div>
        <div class="text-xs text-base-content/50">{session.messages} messages &middot; {session.duration}</div>
      </div>
      <span class="text-sm text-base-content/40">{session.time}</span>
    </div>
  {/each}
</div>

<!-- Cleanup -->
<div class="mb-7">
  <h3 class="text-base font-semibold mb-3">Cleanup</h3>
  <div class="flex flex-col gap-1.5">
    {#each ['30 days', '90 days', '180 days'] as period}
      <div class="flex items-center justify-between p-3 rounded-lg border border-base-content/5 bg-base-100">
        <span class="text-sm">Delete sessions older than {period}</span>
        <button class="px-3 py-1 rounded-md border border-error/20 text-sm text-error cursor-pointer bg-transparent hover:bg-error/5 transition-colors">Delete</button>
      </div>
    {/each}
  </div>
</div>
