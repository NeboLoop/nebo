<!--
  A run's own URL, kept so an audit trail can be linked and shared. It opens
  the run in the modal over the workspace rather than a page of its own.
-->
<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';

  $effect(() => {
    const id = $page.params.agentId;
    const runId = $page.params.runId;
    if (!id || !runId) return;
    goto(`/${id}/threads?run=${encodeURIComponent(runId)}`, { replaceState: true });
  });
</script>
