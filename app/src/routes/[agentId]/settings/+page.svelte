<script lang="ts">
  import { page } from '$app/stores';
  import { goto } from '$lib/nav';

  $effect(() => {
    goto(`/${$page.params.agentId}/settings/general`, { replaceState: true });
  });
</script>
