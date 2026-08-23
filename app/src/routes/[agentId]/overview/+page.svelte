<!--
  The old app landing. Apps open a conversation like every other employee now
  (Open App lives in the chat header) — this redirect keeps old links working.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';

  $effect(() => {
    const id = $page.params.agentId;
    if (id) goto(`/${id}/threads`, { replaceState: true });
  });
</script>
