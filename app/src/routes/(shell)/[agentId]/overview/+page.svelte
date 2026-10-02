<!--
  The old app landing. Apps open a conversation like every other employee now
  (Open App lives in the chat header) — this redirect keeps old links working.
  It goes through /{id}, which resolves the latest conversation: sending it to
  /threads (the NEW-chat page) made the first click on an app open "New chat".
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';

  $effect(() => {
    const id = $page.params.agentId;
    if (id) goto(`/${id}`, { replaceState: true });
  });
</script>
