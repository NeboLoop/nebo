<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';

  $effect(() => {
    goto(`/${$page.params.agentId}/threads`, { replaceState: true });
  });
</script>
