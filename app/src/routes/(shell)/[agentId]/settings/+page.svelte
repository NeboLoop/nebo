<!--
  Employee settings moved into a modal over the workspace, so the conversation
  stays mounted behind it. This route is kept because three places deep-link
  into it (the install flow's configure step, the apps grid kebab menu, and
  agent creation) — it redirects to the modal.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';

  $effect(() => {
    const id = $page.params.agentId;
    if (!id) return;
    goto(`/${id}/threads?settings=general`, { replaceState: true });
  });
</script>
