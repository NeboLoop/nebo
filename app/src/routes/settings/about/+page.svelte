<script lang="ts">
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import { onMount } from 'svelte';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import { updateState, checkForUpdates, setApplying } from '$lib/stores/update';
  import { addToast } from '$lib/stores/toast';

  let version = $state('—');
  let platform = $state('—');
  let licensesText = $state('');
  let showLicenses = $state(false);
  let checkingUpdate = $state(false);

  async function runUpdateCheck() {
    checkingUpdate = true;
    await checkForUpdates();
    checkingUpdate = false;
  }

  async function installUpdate() {
    setApplying();
    try {
      const api = await import('$lib/api/nebo');
      await api.updateApply();
    } catch {
      addToast('Failed to apply update', 'error');
    }
  }

  onMount(async () => {
    try {
      const resp = await fetch('/health');
      const data = await resp.json();
      if (data?.version) version = data.version;
    } catch { /* keep placeholder */ }
    if (typeof navigator !== 'undefined') {
      const ua = navigator.userAgent;
      if (ua.includes('Mac')) platform = 'macOS';
      else if (ua.includes('Windows')) platform = 'Windows';
      else if (ua.includes('Linux')) platform = 'Linux';
      else platform = navigator.platform || '—';
    }
  });

  async function toggleLicenses() {
    if (!licensesText) {
      try {
        const resp = await fetch('/LICENSES.txt');
        licensesText = await resp.text();
      } catch { licensesText = 'Failed to load license information.'; }
    }
    showLicenses = !showLicenses;
  }

  const resources = [
    { label: 'Documentation', url: 'https://docs.neboai.com' },
    { label: 'Report an Issue', url: 'https://github.com/NeboLoop/nebo/issues' },
    { label: 'Privacy Policy', url: 'https://neboai.com/privacy' },
    { label: 'Terms of Service', url: 'https://neboai.com/terms' },
  ];
</script>

<SettingsHeader title="About" description="Application information and resources." />

<div class="p-4 rounded-xl border border-base-content/5 bg-base-100 mb-6">
  <div class="flex items-center gap-3 mb-4">
    <div class="w-12 h-12 rounded-xl bg-base-content text-base-100 grid place-items-center font-mono text-lg font-semibold">N</div>
    <div>
      <div class="text-sm font-bold">Nebo</div>
      <div class="text-xs text-base-content/70">Personal Desktop AI Companion</div>
    </div>
  </div>

  <div class="flex flex-col gap-2 text-sm">
    <div class="flex justify-between py-1.5 border-b border-base-content/5">
      <span class="text-xs text-base-content/70">Version</span>
      <span class="text-xs font-mono">{version}</span>
    </div>
    <div class="flex justify-between py-1.5 border-b border-base-content/5">
      <span class="text-xs text-base-content/70">Platform</span>
      <span class="text-xs font-mono">{platform}</span>
    </div>
    <div class="flex justify-between items-center py-1.5">
      {#if $updateState.available}
        <span class="text-xs text-base-content/70">v{$updateState.latestVersion} available</span>
        <button class="btn btn-primary btn-xs" onclick={installUpdate} disabled={$updateState.applying}>
          {$updateState.applying ? 'Restarting…' : $updateState.ready ? 'Relaunch to update' : 'Install & Restart'}
        </button>
      {:else}
        <span class="text-xs text-base-content/50">{checkingUpdate ? 'Checking…' : 'Updates'}</span>
        <button class="btn btn-ghost btn-xs" onclick={runUpdateCheck} disabled={checkingUpdate}>Check for Updates</button>
      {/if}
    </div>
    {#if $updateState.error}
      <div class="text-xs text-error">{$updateState.error}</div>
    {/if}
  </div>
</div>

<!-- Resources -->
<div class="mb-6">
  <h3 class="text-sm font-semibold mb-3">Resources</h3>
  <div class="flex flex-col gap-1.5">
    {#each resources as resource}
      <a
        href={resource.url}
        target="_blank"
        rel="noopener noreferrer"
        class="flex items-center justify-between p-3 rounded-lg border border-base-content/5 bg-base-100 cursor-pointer hover:border-base-content/15 transition-colors no-underline"
      >
        <span class="text-sm font-medium">{resource.label}</span>
        <ExternalLink class="w-3.5 h-3.5 text-base-content/50" />
      </a>
    {/each}
  </div>
</div>

<!-- Open Source Licenses -->
<div>
  <button
    type="button"
    class="flex items-center justify-between w-full p-3 rounded-lg border border-base-content/5 bg-base-100 cursor-pointer hover:border-base-content/15 transition-colors"
    onclick={toggleLicenses}
  >
    <span class="text-sm font-medium">Open Source Licenses</span>
    <ChevronDown class="w-3.5 h-3.5 text-base-content/50 transition-transform {showLicenses ? 'rotate-180' : ''}" />
  </button>
  {#if showLicenses}
    <textarea
      readonly
      class="w-full h-64 mt-2 p-3 rounded-lg border border-base-content/5 bg-base-200/50 text-xs font-mono text-base-content/70 resize-y outline-none"
      value={licensesText}
    ></textarea>
  {/if}
</div>
