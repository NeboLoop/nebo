<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { t } from 'svelte-i18n';
  import { devMode, appDeveloperSetting, setDevMode, setAppDeveloperMode } from '$lib/stores/devmode.js';

  // Both switches are the bot's settings (every device sees the same):
  // Developer mode shows the advanced settings; App Developer mode, under it,
  // the developer tools for employees that build apps, and a console in each
  // open app.
  let saving = $state(false);

  async function toggle(set: (on: boolean) => Promise<void>, on: boolean) {
    saving = true;
    try {
      await set(on);
    } finally {
      saving = false;
    }
  }
</script>

<SettingsHeader title={$t('settingsDeveloper.title')} description={$t('settingsDeveloper.pageDescription')} />

<!-- Dev mode toggle -->
<div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-2">
  <div class="flex items-center justify-between">
    <div>
      <div class="text-sm font-semibold">{$t('settingsDeveloper.devMode')}</div>
      <div class="text-xs text-base-content/50">{$t('settingsDeveloper.devModeHint')}</div>
    </div>
    <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={$devMode} disabled={saving} onchange={() => toggle(setDevMode, !$devMode)} aria-label={$t('settingsDeveloper.devMode')} />
  </div>
</div>

<p class="text-sm text-base-content/40 mb-6">{$t('settingsDeveloper.defaultRoutingNote')}</p>

{#if $devMode}
  <!-- App Developer mode -->
  <div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-6">
    <div class="flex items-center justify-between gap-4">
      <div>
        <div class="text-sm font-semibold">{$t('settingsDeveloper.appDevMode')}</div>
        <div class="text-xs text-base-content/50">{$t('settingsDeveloper.appDevModeHint')}</div>
      </div>
      <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={$appDeveloperSetting} disabled={saving} onchange={() => toggle(setAppDeveloperMode, !$appDeveloperSetting)} aria-label={$t('settingsDeveloper.appDevMode')} />
    </div>
  </div>
{/if}
