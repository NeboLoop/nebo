<script lang="ts">
  import { t } from 'svelte-i18n';
  import Upload from 'lucide-svelte/icons/upload';
  import FolderOpen from 'lucide-svelte/icons/folder-open';
  import TriangleAlert from 'lucide-svelte/icons/triangle-alert';

  // A pack arrives one of two ways — a .zip the owner drops or picks, or a
  // folder already on this machine. Both go to the same validator, and the
  // validator's answer is shown before anything lands.
  let {
    busy = false,
    error = '',
    notes = [],
    onZip,
    onPath,
    onPickFolder,
  }: {
    busy?: boolean;
    error?: string;
    notes?: string[];
    onZip: (file: File) => void | Promise<void>;
    onPath: (path: string) => void | Promise<void>;
    onPickFolder: () => void | Promise<void>;
  } = $props();

  let path = $state('');
  let over = $state(false);
  let fileInput: HTMLInputElement | null = $state(null);

  function take(list: FileList | null | undefined) {
    const file = list?.[0];
    if (file) void onZip(file);
  }

  function onDrop(e: DragEvent) {
    e.preventDefault();
    over = false;
    take(e.dataTransfer?.files);
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="rounded-lg border border-dashed px-4 py-6 text-center transition-colors {over
    ? 'border-primary bg-primary/5'
    : 'border-base-300 bg-base-100'}"
  ondragover={(e) => { e.preventDefault(); over = true; }}
  ondragleave={() => (over = false)}
  ondrop={onDrop}
>
  <Upload class="w-5 h-5 mx-auto mb-2 text-base-content/40" />
  <div class="text-sm font-medium mb-0.5">{$t('settingsLayers.uploadTitle')}</div>
  <p class="text-xs text-base-content/60 mb-3">{$t('settingsLayers.uploadHint')}</p>
  <button class="btn btn-sm" disabled={busy} onclick={() => fileInput?.click()}>
    {busy ? $t('settingsLayers.validating') : $t('settingsLayers.chooseZip')}
  </button>
  <input
    bind:this={fileInput}
    type="file"
    accept=".zip,application/zip"
    class="hidden"
    onchange={(e) => take((e.currentTarget as HTMLInputElement).files)}
  />
</div>

<div class="flex gap-2 mt-2">
  <input
    type="text"
    class="input input-bordered input-sm flex-1 font-mono"
    placeholder={$t('settingsLayers.pathPlaceholder')}
    bind:value={path}
    onkeydown={(e) => e.key === 'Enter' && path.trim() && onPath(path.trim())}
  />
  <button class="btn btn-sm" disabled={busy} onclick={() => onPickFolder()}>
    <FolderOpen class="w-3.5 h-3.5" />
    {$t('settingsLayers.browse')}
  </button>
  <button class="btn btn-sm btn-primary" disabled={busy || !path.trim()} onclick={() => onPath(path.trim())}>
    {$t('settingsLayers.addFolder')}
  </button>
</div>

{#if error}
  <div class="alert alert-error text-xs mt-2">
    <TriangleAlert class="w-4 h-4 shrink-0" />
    <span>{error}</span>
  </div>
{/if}

{#if notes.length > 0}
  <div class="mt-2 rounded-lg border border-base-300 bg-base-200/40 px-3 py-2">
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1">
      {$t('settingsLayers.validatorFound')}
    </div>
    <ul class="text-xs text-base-content/70 flex flex-col gap-0.5">
      {#each notes as note, i (i)}<li>{note}</li>{/each}
    </ul>
  </div>
{/if}
