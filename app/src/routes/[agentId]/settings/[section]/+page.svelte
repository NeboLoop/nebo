<!--
  Redirects /{agentId}/settings/{section} into the settings modal. Kept so
  every existing deep link and bookmark still lands on the right section.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';

  $effect(() => {
    const id = $page.params.agentId;
    const section = $page.params.section || 'general';
    if (!id) return;
    goto(`/${id}/threads?settings=${encodeURIComponent(section)}`, { replaceState: true });
  });
</script>
