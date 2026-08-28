<script lang="ts">
  import { goto } from '$lib/nav';
  import { onMount } from 'svelte';

  onMount(async () => {
    // Startup lands on the assistant's most recent conversation — the same
    // behavior as clicking its row (selectAgent opens the latest thread).
    // Landing on the blank new-chat composer made every launch look like the
    // existing conversation was gone. No chats yet → the composer is right.
    try {
      const api = await import('$lib/api/nebo');
      const r = await api.listAgentChats('assistant').catch(() => null);
      const latest = r?.chats?.[0]?.id;
      goto(latest ? `/assistant/threads/${latest}` : '/assistant/threads', { replaceState: true });
    } catch {
      goto('/assistant/threads', { replaceState: true });
    }
  });
</script>

<div class="flex items-center justify-center h-full">
  <span class="loading loading-spinner loading-md"></span>
</div>
