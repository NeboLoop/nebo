<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { t } from 'svelte-i18n';
  import GraduationCap from 'lucide-svelte/icons/graduation-cap';
  import Layers from 'lucide-svelte/icons/layers';
  import TriangleAlert from 'lucide-svelte/icons/triangle-alert';
  import SettingsHeader from '$lib/components/settings/SettingsHeader.svelte';
  import StatCard from '$lib/components/settings/StatCard.svelte';
  import DiffView from '$lib/components/settings/layers/DiffView.svelte';
  import LayerFiles from '$lib/components/settings/layers/LayerFiles.svelte';
  import PackUpload from '$lib/components/settings/layers/PackUpload.svelte';
  import SeatProgress from '$lib/components/settings/layers/SeatProgress.svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { formatRelative } from '$lib/time';
  import type { ChangeKind } from '$lib/layers/diff';
  import {
    listLayers,
    listLayerFiles,
    readLayerFile,
    writeLayerFile,
    deleteLayerFile,
    uploadLayerPack,
    applyLayers,
    listLayerSeats,
    pickFolder,
    type LayerPack,
    type LayerFile,
    type LayerFolderCounts,
    type LayerSeat,
    type LayerSeatTally,
    type LayerUploadResponse,
    type PendingLayerEntry,
  } from '$lib/api/nebo';
  import { uploadLayerZip } from '$lib/api/upload';

  // The one place the owner manages the three layers: what they are, what is in
  // them, what changed, and the button that tells the employees to learn it.
  //
  // Initial state comes over REST; updates arrive on the WebSocket rail — the
  // pack watcher's `layers_changed`, which fires only when the parked list
  // actually moved, and each seat's `chat_complete` on its `agent:<id>:layers`
  // session while an update run is out. Nothing here polls.
  //
  // Every write parks. Nothing reaches an employee until the owner applies —
  // an uploaded pack no more than an edited file.

  /** Lowest to highest. The higher layer wins where two speak. */
  const ORDER = ['industry', 'franchise', 'company'];
  /** The three words the loader uses for a change. */
  const KINDS = ['added', 'changed', 'removed'];

  let packs = $state<LayerPack[]>([]);
  let pending = $state<PendingLayerEntry[]>([]);
  let tally = $state<LayerSeatTally | null>(null);
  let seats = $state<LayerSeat[]>([]);
  let files = $state<LayerFile[]>([]);
  let selected = $state('');
  let loading = $state(true);
  let error = $state('');
  let uploading = $state(false);
  let uploadError = $state('');
  let uploadNotes = $state<string[]>([]);
  let teaching = $state(false);
  /** How many employees the last apply sent an update run to; -1 before one. */
  let sentTo = $state(-1);

  const stacked = $derived([...packs].sort((a, b) => rank(a) - rank(b)));
  const selectedPack = $derived(stacked.find((p) => p.slug === selected) ?? null);
  const nothingYet = $derived(!loading && packs.length === 0 && pending.length === 0);
  /** Every enabled employee reads a layer change — the count the button promises. */
  const willRead = $derived(tally?.total ?? 0);

  function rank(pack: LayerPack): number {
    return ORDER.indexOf(pack.layer);
  }

  onMount(() => {
    void refresh();
    void refreshSeats();
  });

  // The parked list moved: a pack landed, an edit parked, or an apply cleared it.
  onWsEvent<{ pending?: string[]; applied?: string[]; seats?: number }>('layers_changed', () => {
    void refresh();
    void refreshSeats();
  });

  // A seat finished its update run: its session is `agent:<id>:layers`.
  onWsEvent<{ session_id?: string }>('chat_complete', (data) => {
    if (data?.session_id?.endsWith(':layers')) void refreshSeats();
  });

  // Files follow the selected layer. The load writes `files`, so it runs
  // untracked — an effect that re-reads what its loader writes is how a page
  // ends up asking fifteen times a second.
  $effect(() => {
    const slug = selected;
    if (!slug) return;
    untrack(() => void loadFiles(slug));
  });

  async function refresh() {
    try {
      const resp = await listLayers();
      packs = resp.packs;
      pending = resp.pending;
      tally = resp.seats;
      error = '';
      if (!packs.some((p) => p.slug === selected)) {
        // The company layer is the one the owner writes, so it opens first.
        const highest = [...packs].sort((a, b) => rank(a) - rank(b)).pop();
        selected = highest?.slug ?? '';
      }
    } catch (e) {
      error = message(e);
    } finally {
      loading = false;
    }
  }

  async function refreshSeats() {
    try {
      const resp = await listLayerSeats();
      seats = resp.seats;
    } catch {
      /* progress is a read; a failed one leaves the last answer on screen */
    }
  }

  async function loadFiles(slug: string) {
    try {
      const resp = await listLayerFiles(slug);
      files = resp.files;
    } catch (e) {
      files = [];
      error = message(e);
    }
  }

  async function openFile(path: string): Promise<string> {
    const resp = await readLayerFile(selected, path);
    return resp.content;
  }

  // Throws on refusal: a save that would stop the pack loading is refused by the
  // backend, and the words belong next to the file, not at the top of the page.
  async function saveFile(path: string, content: string) {
    await writeLayerFile(selected, { path, content });
    // The write parks a change of its own — or un-parks one, when the file goes
    // back to what the employees already read. Either way the overview says so.
    await parked();
  }

  async function removeFile(path: string) {
    await deleteLayerFile(selected, path);
    await loadFiles(selected);
    await parked();
  }

  async function uploadZip(file: File) {
    await staged(() => uploadLayerZip(file));
  }

  async function uploadPath(path: string) {
    await staged(() => uploadLayerPack({ path }));
  }

  async function browseForFolder() {
    try {
      const picked = await pickFolder();
      if (picked?.path) await uploadPath(picked.path);
    } catch (e) {
      uploadError = message(e);
    }
  }

  /** Both upload routes end here: what the check found, or why it refused. */
  async function staged(send: () => Promise<LayerUploadResponse>) {
    uploading = true;
    uploadError = '';
    uploadNotes = [];
    try {
      uploadNotes = describe(await send());
      await parked();
      await refreshSeats();
    } catch (e) {
      uploadError = message(e);
    } finally {
      uploading = false;
    }
  }

  /** What the validator read out of the pack before it landed. */
  function describe(found: LayerUploadResponse): string[] {
    const head = [layerName(found.layer), found.slug];
    if (found.version) head.push('v' + found.version);
    const lines = [head.filter(Boolean).join(' · '), folderLine(found.counts)];
    // An uploaded pack parks like any other change: it waits for the button.
    if (found.pending) lines.push($t('settingsLayers.uploadParked'));
    return lines;
  }

  /** `{ rules: 4, laws: 0, … }` → one line naming only the folders that carry files. */
  function folderLine(counts: LayerFolderCounts): string {
    const named = Object.entries(counts)
      .filter(([, n]) => n > 0)
      .map(([folder, n]) => `${folder}: ${n}`);
    return named.length > 0 ? named.join(' · ') : $t('settingsLayers.noTypedFolders');
  }

  /** A write changed what is parked, so the last apply is no longer the news. */
  async function parked() {
    sentTo = -1;
    await refresh();
  }

  async function teach() {
    teaching = true;
    try {
      const done = await applyLayers({ slugs: pending.map((p) => p.slug) });
      sentTo = done.seats;
      await refresh();
      await refreshSeats();
    } catch (e) {
      error = message(e);
    } finally {
      teaching = false;
    }
  }

  function message(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
  }

  function layerName(layer: string): string {
    return ORDER.includes(layer) ? $t('settingsLayers.layerNames.' + layer) : layer;
  }

  function layerBlurb(layer: string): string {
    return ORDER.includes(layer) ? $t('settingsLayers.layerBlurbs.' + layer) : '';
  }

  function kindLabel(kind: string): string {
    return KINDS.includes(kind) ? $t('settingsLayers.changeKind.' + kind) : kind;
  }

  /** What the pending entry says happened, for the text that carries no markers. */
  function diffKind(kind: string): ChangeKind | undefined {
    return KINDS.includes(kind) ? (kind as ChangeKind) : undefined;
  }
</script>

<SettingsHeader title={$t('settingsLayers.title')} description={$t('settingsLayers.description')} />

{#if error}
  <div class="alert alert-error text-xs mb-5">
    <TriangleAlert class="w-4 h-4 shrink-0" />
    <span>{error}</span>
  </div>
{/if}

{#if loading}
  <div class="flex flex-col gap-2">
    <div class="skeleton h-20 w-full"></div>
    <div class="skeleton h-20 w-full"></div>
    <div class="skeleton h-20 w-full"></div>
  </div>
{:else if nothingYet}
  <!-- A company with no layers is told what a layer is, and offered the upload. -->
  <div class="rounded-lg border border-base-300 bg-base-100 p-5 mb-4">
    <Layers class="w-5 h-5 text-base-content/40 mb-2" />
    <h3 class="text-base font-semibold mb-1">{$t('settingsLayers.emptyTitle')}</h3>
    <p class="text-xs text-base-content/70 leading-relaxed mb-3">{$t('settingsLayers.emptyBody')}</p>
    <div class="flex flex-col gap-1.5">
      {#each ORDER as layer (layer)}
        <div class="flex items-baseline gap-2">
          <span class="text-xs font-semibold w-20 shrink-0">{layerName(layer)}</span>
          <span class="text-xs text-base-content/60">{layerBlurb(layer)}</span>
        </div>
      {/each}
    </div>
    <p class="text-xs text-base-content/70 mt-3">{$t('settingsLayers.precedence')}</p>
  </div>

  <PackUpload
    busy={uploading}
    error={uploadError}
    notes={uploadNotes}
    onZip={uploadZip}
    onPath={uploadPath}
    onPickFolder={browseForFolder}
  />
{:else}
  <!-- 1. The layers, as they are, lowest to highest. -->
  <div class="mb-7">
    <h3 class="text-base font-semibold mb-3">{$t('settingsLayers.stackTitle')}</h3>
    <div class="flex flex-col gap-1.5">
      {#each stacked as pack, i (pack.slug)}
        <button
          class="w-full text-left p-3.5 rounded-lg border transition-colors cursor-pointer {pack.slug === selected
            ? 'border-primary bg-primary/5'
            : 'border-base-300 bg-base-100 hover:bg-base-200'}"
          onclick={() => (selected = pack.slug)}
        >
          <div class="flex items-center gap-2 mb-0.5">
            <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">
              {layerName(pack.layer)}
            </span>
            {#if i === stacked.length - 1}
              <span class="px-1.5 py-0.5 rounded text-xs font-medium bg-primary/10 text-primary">
                {$t('settingsLayers.topLayer')}
              </span>
            {/if}
            <span class="ml-auto text-xs text-base-content/50">
              {pack.updatedAt
                ? $t('settingsLayers.changedAt', { values: { when: formatRelative(pack.updatedAt) } })
                : $t('settingsLayers.neverChanged')}
            </span>
          </div>
          <div class="flex items-center gap-2 min-w-0">
            <span class="text-sm font-semibold truncate">{pack.name}</span>
            {#if pack.version}
              <span class="shrink-0 px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70">
                v{pack.version}
              </span>
            {/if}
          </div>
          <div class="flex items-center gap-2 mt-0.5 flex-wrap">
            <span class="text-xs text-base-content/50">
              {$t('settingsLayers.fileCount', { values: { n: pack.fileCount } })}
            </span>
            <span class="text-xs font-mono text-base-content/50 truncate">{pack.stamp}</span>
          </div>
        </button>
      {/each}
    </div>
    <p class="text-xs text-base-content/70 mt-2">{$t('settingsLayers.precedence')}</p>
  </div>

  <!-- 2. Browse and edit the selected layer. -->
  {#if selectedPack}
    <div class="mb-7">
      <h3 class="text-base font-semibold mb-3">
        {$t('settingsLayers.filesTitle', { values: { name: selectedPack.name } })}
      </h3>
      <LayerFiles
        {files}
        fromMarketplace={selectedPack.layer !== 'company'}
        onOpen={openFile}
        onSave={saveFile}
        onDelete={removeFile}
      />
    </div>
  {/if}

  <!-- 3. Add a layer. -->
  <div class="mb-7">
    <h3 class="text-base font-semibold mb-3">{$t('settingsLayers.uploadSection')}</h3>
    <PackUpload
      busy={uploading}
      error={uploadError}
      notes={uploadNotes}
      onZip={uploadZip}
      onPath={uploadPath}
      onPickFolder={browseForFolder}
    />
  </div>

  <!-- 4. What changed, as a diff. -->
  <div class="mb-7">
    <h3 class="text-base font-semibold mb-3">{$t('settingsLayers.changesTitle')}</h3>
    {#if pending.length === 0}
      <div class="rounded-lg border border-base-300 px-4 py-6 text-center text-xs text-base-content/60">
        {$t('settingsLayers.changesNone')}
      </div>
    {:else}
      <div class="flex flex-col gap-4">
        {#each pending as change (change.slug + change.stamp)}
          <div>
            <div class="flex items-center gap-2 mb-1.5 flex-wrap">
              <span class="text-xs font-semibold uppercase tracking-wider text-base-content/50">
                {layerName(change.layer)}
              </span>
              <span class="text-sm font-semibold">{change.name}</span>
              {#if change.kind}
                <span class="px-1.5 py-0.5 rounded text-xs font-medium bg-warning/10 text-warning">
                  {kindLabel(change.kind)}
                </span>
              {/if}
              {#if change.detectedAt}
                <span class="ml-auto text-xs text-base-content/40">{formatRelative(change.detectedAt)}</span>
              {/if}
            </div>
            <div class="text-xs font-mono text-base-content/50 mb-2 truncate">
              {change.previousStamp
                ? $t('settingsLayers.stampMoved', {
                    values: { from: change.previousStamp, to: change.stamp },
                  })
                : $t('settingsLayers.stampNew', { values: { to: change.stamp } })}
            </div>
            <DiffView text={change.diff} kind={diffKind(change.kind)} />
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <!-- 5. Teach the employees. The whole point of the screen. -->
  <div class="mb-4">
    <h3 class="text-base font-semibold mb-3">{$t('settingsLayers.teachTitle')}</h3>
    {#if pending.length > 0}
      <div class="rounded-lg border border-warning/30 bg-warning/5 p-4 mb-3">
        <p class="text-xs text-base-content/80 mb-3">
          {$t('settingsLayers.notReachedYet', { values: { n: willRead } })}
        </p>
        <button class="btn btn-sm btn-primary" disabled={teaching} onclick={teach}>
          <GraduationCap class="w-3.5 h-3.5" />
          {teaching
            ? $t('settingsLayers.teaching')
            : $t('settingsLayers.teachButton', { values: { n: willRead } })}
        </button>
      </div>
    {:else}
      <p class="text-xs text-base-content/70 mb-3">
        {sentTo >= 0
          ? $t('settingsLayers.sentCount', { values: { n: sentTo } })
          : $t('settingsLayers.allCaughtUp')}
      </p>
    {/if}

    {#if tally}
      <div class="flex gap-3 mb-3">
        <StatCard label={$t('settingsLayers.seatStatus.written')} value={tally.written} accent="success" />
        <StatCard label={$t('settingsLayers.seatStatus.pending')} value={tally.pending} />
        <StatCard label={$t('settingsLayers.seatStatus.stale')} value={tally.stale} />
      </div>
    {/if}

    {#if seats.length > 0}
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-1.5">
        {$t('settingsLayers.seatsTitle')}
      </div>
      <SeatProgress {seats} />
    {/if}
  </div>
{/if}
