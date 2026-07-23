<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { browserStatus } from '$lib/api/nebo';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import RefreshCw from 'lucide-svelte/icons/refresh-cw';
  import CheckCircle2 from 'lucide-svelte/icons/check-circle-2';
  import XCircle from 'lucide-svelte/icons/x-circle';

  const CHROME_EXTENSION_URL =
    'https://chromewebstore.google.com/detail/nebo-browser-relay/heaeiepdllbncnnlfniglgmbfmmemkcg';

  let extensionConnected = $state(false);
  let builtInAvailable = $state(false);
  let loading = $state(true);

  async function refresh() {
    loading = true;
    try {
      const status = await browserStatus();
      extensionConnected = status?.extensionConnected ?? false;
      builtInAvailable = Boolean(status?.builtInAvailable);
    } catch {
      extensionConnected = false;
      builtInAvailable = false;
    } finally {
      loading = false;
    }
  }

  onMount(refresh);
</script>

<SettingsHeader
  title={$t('settingsBrowser.title')}
  description={$t('settingsBrowser.description')}
/>

<!-- Extension status -->
<div class="p-4 rounded-xl border border-base-content/5 bg-base-100 mb-4">
  <div class="flex items-center justify-between gap-3">
    <div class="flex items-center gap-3">
      {#if extensionConnected}
        <CheckCircle2 class="w-5 h-5 text-success shrink-0" />
      {:else}
        <XCircle class="w-5 h-5 text-base-content/40 shrink-0" />
      {/if}
      <div>
        <div class="text-sm font-medium">{$t('settingsBrowser.extension')}</div>
        <div class="text-xs text-base-content/70">
          {#if loading}
            {$t('common.loading')}
          {:else if extensionConnected}
            {$t('settingsBrowser.extensionConnected')}
          {:else}
            {$t('browserExtension.notConnected')}
          {/if}
        </div>
      </div>
    </div>
    <button
      type="button"
      class="btn btn-ghost btn-sm gap-1.5 shrink-0"
      onclick={refresh}
      disabled={loading}
      aria-label={$t('browserExtension.retry')}
    >
      <RefreshCw class="w-3.5 h-3.5 {loading ? 'animate-spin' : ''}" />
      {$t('browserExtension.retry')}
    </button>
  </div>

  {#if !extensionConnected && !loading}
    <p class="text-xs text-base-content/70 mt-3">{$t('browserExtension.instructions')}</p>
    <a
      href={CHROME_EXTENSION_URL}
      target="_blank"
      rel="noopener noreferrer"
      class="btn btn-primary btn-sm gap-1.5 mt-3 no-underline"
    >
      {$t('browserExtension.install')}
      <ExternalLink class="w-3.5 h-3.5" />
    </a>
  {/if}
</div>

<!-- Built-in fallback status -->
<div class="p-4 rounded-xl border border-base-content/5 bg-base-100">
  <div class="flex items-center gap-3">
    {#if builtInAvailable}
      <CheckCircle2 class="w-5 h-5 text-success shrink-0" />
    {:else}
      <XCircle class="w-5 h-5 text-base-content/40 shrink-0" />
    {/if}
    <div>
      <div class="text-sm font-medium">{$t('settingsBrowser.builtIn')}</div>
      <div class="text-xs text-base-content/70">
        {#if loading}
          {$t('common.loading')}
        {:else if builtInAvailable}
          {$t('settingsBrowser.builtInAvailable')}
        {:else}
          {$t('settingsBrowser.builtInMissing')}
        {/if}
      </div>
    </div>
  </div>
</div>
