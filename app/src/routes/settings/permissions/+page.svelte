<!--
  Settings → Permissions: the company defaults every employee inherits, with
  the same controls as an employee's own Permissions page.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import PermissionsSection from '$lib/components/settings/agent/PermissionsSection.svelte';
</script>

<SettingsHeader title={$t('permissions.title')} description={$t('permissions.companyDescription')} />

<PermissionsSection />
