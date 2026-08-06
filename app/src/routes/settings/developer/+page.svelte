<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { devMode } from '$lib/stores/devmode.js';

  let appPath = $state('');

  // Loop-guardrail thresholds (settings.guardrails blob; defaults mirror
  // agent::guardrails::GuardrailConfig).
  const GUARDRAIL_DEFAULTS = {
    sameActionLimit: 8,
    identicalArgsBlockAfter: 3,
    maxAutoContinuations: 5,
    hardStop: false,
  };
  let guardrails = $state({ ...GUARDRAIL_DEFAULTS });
  let guardrailsStatus = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const resp = (await api.getSettings()) as { settings?: { guardrails?: Record<string, unknown> } };
      guardrails = { ...GUARDRAIL_DEFAULTS, ...(resp?.settings?.guardrails ?? {}) };
    } catch {
      // keep defaults
    }
  });

  async function saveGuardrails() {
    guardrailsStatus = 'saving';
    try {
      const api = await import('$lib/api/nebo');
      await api.updateSettings({ guardrails });
      guardrailsStatus = 'saved';
    } catch {
      guardrailsStatus = 'error';
    }
    setTimeout(() => (guardrailsStatus = 'idle'), 2500);
  }

  function resetGuardrails() {
    guardrails = { ...GUARDRAIL_DEFAULTS };
    void saveGuardrails();
  }

  const sideloadedApps = [
    { name: 'My Custom Tool', path: '~/projects/custom-tool', status: 'running' as const },
    { name: 'Test Plugin', path: '~/projects/test-plugin', status: 'stopped' as const },
  ];
</script>

<SettingsHeader title={$t('settingsDeveloper.title')} description={$t('settingsDeveloper.pageDescription')} />

<!-- Dev mode toggle -->
<div class="p-4 rounded-xl border border-base-content/10 bg-base-100 mb-2">
  <div class="flex items-center justify-between">
    <div>
      <div class="text-sm font-semibold">{$t('settingsDeveloper.devMode')}</div>
      <div class="text-xs text-base-content/50">{$t('settingsDeveloper.devModeHint')}</div>
    </div>
    <input type="checkbox" class="toggle toggle-sm toggle-primary" checked={$devMode} onchange={() => $devMode = !$devMode} />
  </div>
</div>

<p class="text-sm text-base-content/40 mb-6">{$t('settingsDeveloper.defaultRoutingNote')}</p>

{#if $devMode}
  <!-- Loop guardrails -->
  <div class="mb-6">
    <h3 class="text-base font-semibold mb-1">{$t('settingsDeveloper.guardrails')}</h3>
    <p class="text-xs text-base-content/50 mb-3">{$t('settingsDeveloper.guardrailsDesc')}</p>
    <div class="p-4 rounded-xl border border-base-content/10 bg-base-100 flex flex-col gap-4">
      <div class="flex items-center justify-between gap-4">
        <div>
          <div class="text-sm font-semibold">{$t('settingsDeveloper.sameActionLimit')}</div>
          <div class="text-xs text-base-content/50">{$t('settingsDeveloper.sameActionLimitDesc')}</div>
        </div>
        <input type="number" min="2" max="100" class="input input-sm input-bordered w-24 text-right" bind:value={guardrails.sameActionLimit} />
      </div>
      <div class="flex items-center justify-between gap-4">
        <div>
          <div class="text-sm font-semibold">{$t('settingsDeveloper.identicalArgsBlockAfter')}</div>
          <div class="text-xs text-base-content/50">{$t('settingsDeveloper.identicalArgsBlockAfterDesc')}</div>
        </div>
        <input type="number" min="1" max="50" class="input input-sm input-bordered w-24 text-right" bind:value={guardrails.identicalArgsBlockAfter} />
      </div>
      <div class="flex items-center justify-between gap-4">
        <div>
          <div class="text-sm font-semibold">{$t('settingsDeveloper.maxAutoContinuations')}</div>
          <div class="text-xs text-base-content/50">{$t('settingsDeveloper.maxAutoContinuationsDesc')}</div>
        </div>
        <input type="number" min="0" max="50" class="input input-sm input-bordered w-24 text-right" bind:value={guardrails.maxAutoContinuations} />
      </div>
      <div class="flex items-center justify-between gap-4">
        <div>
          <div class="text-sm font-semibold">{$t('settingsDeveloper.hardStop')}</div>
          <div class="text-xs text-base-content/50">{$t('settingsDeveloper.hardStopDesc')}</div>
        </div>
        <input type="checkbox" class="toggle toggle-sm toggle-primary" bind:checked={guardrails.hardStop} />
      </div>
      <div class="flex items-center justify-end gap-2 pt-1">
        {#if guardrailsStatus === 'saved'}
          <span class="text-xs text-success">{$t('settingsDeveloper.guardrailsSaved')}</span>
        {:else if guardrailsStatus === 'error'}
          <span class="text-xs text-error">{$t('settingsDeveloper.guardrailsSaveFailed')}</span>
        {/if}
        <button class="px-3 py-1.5 rounded-lg border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors" onclick={resetGuardrails}>{$t('settingsDeveloper.resetDefaults')}</button>
        <button class="px-4 py-1.5 rounded-lg text-sm font-medium cursor-pointer bg-primary text-primary-content hover:opacity-90 transition-opacity disabled:opacity-50" disabled={guardrailsStatus === 'saving'} onclick={saveGuardrails}>{$t('settingsDeveloper.save')}</button>
      </div>
    </div>
  </div>

  <!-- Sideload app -->
  <div class="mb-6">
    <h3 class="text-base font-semibold mb-3">{$t('settingsDeveloper.sideloadApp')}</h3>
    <div class="flex gap-2">
      <input type="text" bind:value={appPath} placeholder={$t('settingsDeveloper.appPathPlaceholder')} class="flex-1 py-2 px-3 rounded-lg border border-base-content/25 bg-base-200/40 text-sm font-mono outline-none focus:border-base-content/50" />
      <button class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer bg-base-100 hover:bg-base-200 transition-colors" disabled={!appPath.trim()}>{$t('settingsDeveloper.load')}</button>
    </div>
  </div>

  <!-- Loaded apps -->
  <div class="mb-6">
    <h3 class="text-base font-semibold mb-3">{$t('settingsDeveloper.sideloadedApps')}</h3>
    <div class="flex flex-col gap-1.5">
      {#each sideloadedApps as app}
        <div class="flex items-center gap-3 p-3.5 rounded-lg border border-base-content/5 bg-base-100">
          <div class="flex-1">
            <div class="flex items-center gap-2 mb-0.5">
              <span class="text-sm font-semibold">{app.name}</span>
              <span class="px-1.5 py-0.5 rounded text-sm font-mono bg-accent/10 text-accent">{$t('common.dev')}</span>
              <span class="px-1.5 py-0.5 rounded text-sm font-mono {app.status === 'running' ? 'bg-success/10 text-success' : 'bg-base-200'}">{app.status}</span>
            </div>
            <div class="text-sm font-mono text-base-content/50">{app.path}</div>
          </div>
          <button class="px-3 py-1 rounded-md border border-base-content/10 text-sm cursor-pointer bg-transparent hover:bg-base-200 transition-colors">{$t('settingsDeveloper.relaunch')}</button>
          <button class="px-3 py-1 rounded-md border border-error/20 text-sm text-error cursor-pointer bg-transparent hover:bg-error/5 transition-colors">{$t('settingsDeveloper.unload')}</button>
        </div>
      {/each}
    </div>
  </div>

  <!-- How it works -->
  <div class="p-4 rounded-lg bg-base-200/50 text-sm leading-relaxed">
    <div class="font-semibold mb-2">{$t('settingsDeveloper.howItWorksTitle')}</div>
    <ul class="list-disc list-inside flex flex-col gap-1 text-base-content/70">
      <li>{$t('settingsDeveloper.sideloadPoint1')}</li>
      <li>{$t('settingsDeveloper.sideloadPoint2')}</li>
      <li>{$t('settingsDeveloper.sideloadPoint3')}</li>
      <li>{$t('settingsDeveloper.sideloadPoint4')}</li>
    </ul>
  </div>
{/if}
