<script lang="ts">
  import SettingsShell from '$lib/components/SettingsShell.svelte';
  import { t } from 'svelte-i18n';
  let { children } = $props();
</script>

<svelte:head><title>{$t('settings.pageTitle')}</title></svelte:head>

<SettingsShell>
  {@render children()}
</SettingsShell>
