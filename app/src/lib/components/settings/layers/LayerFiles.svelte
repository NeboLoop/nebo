<script lang="ts">
  import { t } from 'svelte-i18n';
  import Check from 'lucide-svelte/icons/check';
  import FileText from 'lucide-svelte/icons/file-text';
  import Trash2 from 'lucide-svelte/icons/trash-2';
  import Info from 'lucide-svelte/icons/info';
  import ChevronLeft from 'lucide-svelte/icons/chevron-left';
  import type { LayerFile } from '$lib/api/nebo';
  import TriangleAlert from 'lucide-svelte/icons/triangle-alert';

  // The typed folders of one layer and its marker file. Click a file, read it,
  // edit it in place; it saves as you type. Every layer is editable — the
  // company layer is the owner's own and is never shown as read-only.

  /** The folders a pack may carry, in the order a seat reads them. */
  const FOLDERS = ['vocabulary', 'parties', 'rules', 'laws', 'standards', 'workflows', 'reference'];
  /** How long after the last keystroke the file is written. */
  const SAVE_AFTER_MS = 700;

  let {
    files,
    fromMarketplace = false,
    onOpen,
    onSave,
    onDelete,
  }: {
    files: LayerFile[];
    /** An industry pack installed from the marketplace: editable, with a note. */
    fromMarketplace?: boolean;
    onOpen: (path: string) => Promise<string>;
    onSave: (path: string, content: string) => Promise<void>;
    onDelete: (path: string) => Promise<void>;
  } = $props();

  let openPath = $state('');
  let content = $state('');
  let loading = $state(false);
  let saved = $state(false);
  let saveError = $state('');
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let confirmingDelete = $state(false);

  /** Marker file first, then the typed folders in reading order, then the rest. */
  const groups = $derived.by(() => {
    const byFolder = new Map<string, LayerFile[]>();
    for (const f of files) {
      const parts = f.path.split('/');
      const folder = parts.length > 1 ? parts[0] : '';
      const list = byFolder.get(folder) ?? [];
      list.push(f);
      byFolder.set(folder, list);
    }
    const order = ['', ...FOLDERS];
    const known = order
      .filter((k) => byFolder.has(k))
      .map((k) => ({ folder: k, files: byFolder.get(k)!.sort((a, b) => a.path.localeCompare(b.path)) }));
    const extra = [...byFolder.keys()]
      .filter((k) => !order.includes(k))
      .sort()
      .map((k) => ({ folder: k, files: byFolder.get(k)!.sort((a, b) => a.path.localeCompare(b.path)) }));
    return [...known, ...extra];
  });

  async function open(path: string) {
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; await persist(openPath, content); }
    openPath = path;
    confirmingDelete = false;
    saveError = '';
    loading = true;
    try {
      content = await onOpen(path);
    } catch (e) {
      content = '';
      saveError = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function close() {
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; void persist(openPath, content); }
    openPath = '';
    content = '';
    confirmingDelete = false;
  }

  function edited() {
    if (saveTimer) clearTimeout(saveTimer);
    const path = openPath;
    const text = content;
    saveTimer = setTimeout(() => { saveTimer = null; void persist(path, text); }, SAVE_AFTER_MS);
  }

  // A save the backend refuses (a file that would stop the pack loading) is
  // reported next to the file, where the owner is looking.
  async function persist(path: string, text: string) {
    if (!path) return;
    try {
      await onSave(path, text);
      saveError = '';
      saved = true;
      setTimeout(() => (saved = false), 2000);
    } catch (e) {
      saveError = e instanceof Error ? e.message : String(e);
    }
  }

  async function remove() {
    const path = openPath;
    if (saveTimer) { clearTimeout(saveTimer); saveTimer = null; }
    confirmingDelete = false;
    try {
      await onDelete(path);
      openPath = '';
      content = '';
      saveError = '';
    } catch (e) {
      saveError = e instanceof Error ? e.message : String(e);
    }
  }

  function sizeLabel(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    return `${(bytes / 1024).toFixed(1)} kB`;
  }
</script>

{#if fromMarketplace}
  <div class="flex items-start gap-2 mb-3 text-xs text-base-content/60">
    <Info class="w-3.5 h-3.5 mt-0.5 shrink-0" />
    <span>{$t('settingsLayers.marketplaceEditNote')}</span>
  </div>
{/if}

{#if openPath}
  <div class="flex items-center gap-2 mb-2">
    <button class="btn btn-ghost btn-xs" onclick={close}>
      <ChevronLeft class="w-3.5 h-3.5" />
      {$t('settingsLayers.allFiles')}
    </button>
    <span class="text-xs font-mono font-semibold truncate min-w-0">{openPath}</span>
    <span class="ml-auto flex items-center gap-2 shrink-0">
      {#if saved}
        <span class="text-xs text-success flex items-center gap-1"><Check class="w-3 h-3" /> {$t('common.saved')}</span>
      {/if}
      <button
        class="p-1.5 rounded-md hover:bg-error/10 text-error transition-colors cursor-pointer bg-transparent border-none"
        title={$t('settingsLayers.removeFile')}
        onclick={() => (confirmingDelete = true)}
      >
        <Trash2 class="w-3.5 h-3.5" />
      </button>
    </span>
  </div>

  {#if confirmingDelete}
    <div class="alert alert-warning text-xs mb-2 flex-wrap">
      <span class="flex-1">{$t('settingsLayers.removeConfirm', { values: { path: openPath } })}</span>
      <button class="btn btn-xs" onclick={() => (confirmingDelete = false)}>{$t('common.cancel')}</button>
      <button class="btn btn-xs btn-error" onclick={remove}>{$t('settingsLayers.removeFile')}</button>
    </div>
  {/if}

  {#if saveError}
    <div class="alert alert-error text-xs mb-2">
      <TriangleAlert class="w-4 h-4 shrink-0" />
      <span>{saveError}</span>
    </div>
  {/if}

  {#if loading}
    <div class="skeleton h-56 w-full"></div>
  {:else}
    <textarea
      class="w-full py-2.5 px-3 rounded-lg border border-base-content/25 bg-base-200/40 text-xs outline-none focus:border-base-content/50 resize-y font-mono leading-relaxed"
      rows="18"
      spellcheck="false"
      bind:value={content}
      oninput={edited}
    ></textarea>
    <p class="text-xs text-base-content/50 mt-1">{$t('settingsLayers.savesAsYouType')}</p>
  {/if}
{:else if files.length === 0}
  <div class="rounded-lg border border-base-300 px-4 py-6 text-center text-xs text-base-content/60">
    {$t('settingsLayers.noFiles')}
  </div>
{:else}
  <div class="flex flex-col gap-3">
    {#each groups as group (group.folder)}
      <div>
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">
          {group.folder ? group.folder : $t('settingsLayers.marker')}
        </div>
        <div class="flex flex-col gap-0.5">
          {#each group.files as file (file.path)}
            <button
              class="flex items-center gap-2 py-1.5 px-2.5 rounded-lg text-left hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none w-full"
              onclick={() => open(file.path)}
            >
              <FileText class="w-3.5 h-3.5 text-base-content/40 shrink-0" />
              <span class="text-xs font-mono truncate flex-1 min-w-0">
                {group.folder ? file.path.slice(group.folder.length + 1) : file.path}
              </span>
              <span class="text-xs text-base-content/40 shrink-0">{sizeLabel(file.bytes)}</span>
            </button>
          {/each}
        </div>
      </div>
    {/each}
  </div>
{/if}
