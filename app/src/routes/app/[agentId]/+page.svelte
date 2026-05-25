<script lang="ts">
  import { goto } from '$app/navigation';
  import { page } from '$app/stores';

  const agentId = $derived($page.params.agentId ?? '');

  $effect(() => {
    if (agentId) {
      goto(`/${agentId}/threads`, { replaceState: true });
    }
  });
</script>
