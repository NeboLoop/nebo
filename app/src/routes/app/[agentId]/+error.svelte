<script lang="ts">
  import { page } from '$app/stores';
  import { t } from 'svelte-i18n';
</script>

<div class="h-screen flex items-center justify-center bg-base-100 p-8">
  <div class="text-center max-w-lg">
    <div class="text-base font-semibold mb-2">{$t('apps.appError', { values: { status: $page.status } })}</div>
    <pre class="text-xs text-base-content/70 whitespace-pre-wrap text-start bg-base-200 p-4 rounded-lg overflow-auto max-h-96">{$page.error?.message || $t('settingsUpdates.unknownError')}</pre>
  </div>
</div>
