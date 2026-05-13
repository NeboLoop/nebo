<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';

  $effect(() => {
    goto(`/${$page.params.agentId}/settings/general`, { replaceState: true });
  });
</script>
