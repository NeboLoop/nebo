<script lang="ts">
  import { showUpdateBanner, updateDownloading, updateState, setApplying } from '$lib/stores/update';
  import { addToast } from '$lib/stores/toast';

  let { collapsed = false } = $props();

  async function applyUpdate() {
    setApplying();
    try {
      const api = await import('$lib/api/nebo');
      await api.updateApply();
    } catch (e) {
      addToast('Failed to apply update', 'error');
    }
  }
</script>

{#if $showUpdateBanner}
  <div class="border-t border-base-300 shrink-0">
    <button
      class="w-full flex items-center gap-2.5 cursor-pointer hover:bg-base-200 transition-colors bg-transparent border-none {collapsed ? 'justify-center py-2.5 px-0' : 'py-2.5 px-3.5 text-left'}"
      onclick={applyUpdate}
    >
      <div class="w-7 h-7 rounded-full bg-primary/10 text-primary flex items-center justify-center shrink-0">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
      </div>
      {#if !collapsed}
        <div class="flex-1 min-w-0">
          <div class="text-sm font-medium">Relaunch to update</div>
          <div class="text-xs text-base-content/70">v{$updateState.latestVersion}</div>
        </div>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/50 shrink-0"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
      {/if}
    </button>
  </div>
{:else if $updateDownloading}
  <div class="border-t border-base-300 shrink-0 {collapsed ? 'px-1 py-2' : 'px-3.5 py-2'}">
    {#if !collapsed}
      <div class="text-xs text-base-content/50 mb-1">Downloading update...</div>
    {/if}
    <progress class="progress progress-primary w-full h-1" value={$updateState.downloadPercent} max="100"></progress>
  </div>
{/if}
