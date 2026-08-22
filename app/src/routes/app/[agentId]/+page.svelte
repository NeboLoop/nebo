<script lang="ts">
  import { goto } from '$lib/nav';
  import { page } from '$app/stores';

  const agentId = $derived($page.params.agentId ?? '');

  $effect(() => {
    if (agentId) {
      // /[agentId]/+page.svelte owns the app-vs-agent branching (overview vs threads).
      goto(`/${agentId}`, { replaceState: true });
    }
  });
</script>
