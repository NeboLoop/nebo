<script lang="ts">
  import SettingsShell from '$lib/components/SettingsShell.svelte';
  let { children } = $props();
</script>

<svelte:head><title>Settings - Nebo</title></svelte:head>

<SettingsShell>
  {@render children()}
</SettingsShell>
