<script lang="ts">
  /**
   * WorkViewer — the ONE renderer for Work-panel artifacts, routed by file
   * extension (one renderer per format). Heavy libraries (pdfjs-dist,
   * docx-preview, xlsx, shiki) load on demand via dynamic import so the main
   * bundle stays lean. Fetching lives here too: text formats fetch as text,
   * binary formats as ArrayBuffer, media not at all (the browser streams it).
   *
   * Security model: HTML artifacts run in a sandboxed
   * iframe WITHOUT allow-same-origin (opaque origin — scripts may run but
   * can't reach the app, its API, or its storage). DOCX renders via
   * docx-preview to styled DOM (no scripts/macros execute). Spreadsheets
   * (xlsx/xls) render in SheetView from the sheet engine's view model — the
   * browser never evaluates a formula.
   */
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { backendUrl } from '$lib/api/base';
  import { UPLOAD_KEEP_DAYS } from '$lib/api/upload';
  import { firstFrame } from '$lib/types/attachment';
  import { downloadArtifact } from '$lib/chat/download';
  import SheetView from '$lib/components/sheet/SheetView.svelte';

  let {
    url,
    title,
    renderHtml,
    oncontentclick,
    sourceView = false,
    codeUrl,
    documentId,
    version,
    onsaved,
    agentId,
    sessionKey,
  }: {
    url: string;
    title: string;
    /** The work document behind the file (`path:<url>` for a file opened by its path). Spreadsheets edit through it. */
    documentId?: string;
    version?: number;
    /** A spreadsheet's edits were saved as a new version. */
    onsaved?: (version: number) => void;
    /** The chat a spreadsheet's "Saved …" message lands in (its agent and session key). */
    agentId?: string;
    sessionKey?: string;
    /** Markdown → HTML renderer shared with the chat (mention chips + code-copy buttons). */
    renderHtml: (md: string) => string;
    oncontentclick?: (e: MouseEvent) => void;
    /** Show the artifact's source instead of its rendered form (Preview/Code toggle). */
    sourceView?: boolean;
    /** Source file behind a compiled artifact (the .jsx behind a .html). */
    codeUrl?: string;
  } = $props();

  // Artifact URLs arrive root-relative from backend payloads (/api/v1/files/...)
  // — resolve them through backendBase() so they carry the tunnel prefix.
  const src = $derived(backendUrl(url));
  const codeSrc = $derived(codeUrl ? backendUrl(codeUrl) : undefined);

  const ext = $derived((title.split('.').pop() || '').toLowerCase());

  const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg'];
  const VIDEO_EXTS = ['mp4', 'webm', 'mov'];
  const AUDIO_EXTS = ['mp3', 'm4a', 'wav', 'ogg', 'aac', 'flac'];
  const CODE_LANGS: Record<string, string> = {
    js: 'javascript', mjs: 'javascript', cjs: 'javascript', ts: 'typescript',
    py: 'python', rs: 'rust', go: 'go', json: 'json', sh: 'bash', bash: 'bash',
    css: 'css', yaml: 'yaml', yml: 'yaml', toml: 'toml', sql: 'sql',
    svelte: 'svelte', tsx: 'tsx', jsx: 'jsx', rb: 'ruby', java: 'java', html: 'html',
    c: 'c', h: 'c', cpp: 'cpp', xml: 'xml', md: 'markdown', markdown: 'markdown',
  };

  type Mode =
    | 'markdown' | 'html' | 'pdf' | 'sheet' | 'csv' | 'docx' | 'code'
    | 'pptx' | 'image' | 'video' | 'audio' | 'download';

  const mode: Mode = $derived.by(() => {
    if (ext === 'md' || ext === 'txt') return 'markdown';
    if (ext === 'html' || ext === 'htm') return 'html';
    if (ext === 'pdf') return 'pdf';
    if (ext === 'pptx' || ext === 'ppt') return 'pptx';
    if (ext === 'xlsx' || ext === 'xls') return 'sheet';
    if (ext === 'csv' || ext === 'tsv') return 'csv';
    if (ext === 'docx') return 'docx';
    if (IMAGE_EXTS.includes(ext)) return 'image';
    if (VIDEO_EXTS.includes(ext)) return 'video';
    if (AUDIO_EXTS.includes(ext)) return 'audio';
    if (CODE_LANGS[ext]) return 'code';
    // ppt/pptx/doc and anything else: no faithful in-app preview — offer the file.
    return 'download';
  });

  let loading = $state(true);
  let error = $state('');
  /** Rendered HTML for markdown / docx / code modes. */
  let renderedHtml = $state('');
  /** A markdown file's YAML front matter (name, description, …), shown as a
   *  small metadata block instead of being parsed as a heading. */
  let frontMatter = $state<[string, string][]>([]);

  function splitFrontMatter(text: string): { meta: [string, string][]; body: string } {
    const m = text.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
    if (!m) return { meta: [], body: text };
    const meta: [string, string][] = [];
    for (const line of m[1].split(/\r?\n/)) {
      const kv = line.match(/^([A-Za-z0-9_-]+):\s*(.*)$/);
      if (kv) meta.push([kv[1], kv[2].replace(/^["']|["']$/g, '')]);
    }
    return { meta, body: text.slice(m[0].length) };
  }
  /** Parsed CSV/TSV data: name + rows. */
  let sheets = $state<{ name: string; rows: string[][]; total: number }[]>([]);
  let pdfContainer = $state<HTMLDivElement | null>(null);
  let docxContainer = $state<HTMLDivElement | null>(null);

  const SHEET_ROW_CAP = 500;

  /** Why a file didn't load: one the hub cleared after its keeping period
   *  (410) says so; anything else gives the status. */
  function loadFailed(status: number): string {
    return status === 410
      ? $t('chat.fileRemoved', { values: { days: UPLOAD_KEEP_DAYS } })
      : $t('chat.failedToLoadStatus', { values: { status } });
  }

  async function fetchText(): Promise<string> {
    const res = await fetch(src);
    if (!res.ok) throw new Error(loadFailed(res.status));
    return res.text();
  }

  async function fetchBinary(): Promise<ArrayBuffer> {
    const res = await fetch(src);
    if (!res.ok) throw new Error(loadFailed(res.status));
    return res.arrayBuffer();
  }

  // RFC 4180 parse: quoted fields may contain the separator, "" escapes,
  // and embedded newlines — naive line/sep splitting scrambles real CSV.
  function parseCsv(text: string, sep: string): string[][] {
    const rows: string[][] = [];
    let row: string[] = [];
    let field = '';
    let inQuotes = false;
    const endField = () => {
      row.push(field.trim());
      field = '';
    };
    const endRow = () => {
      endField();
      if (row.some((c) => c)) rows.push(row);
      row = [];
    };
    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      if (inQuotes) {
        if (ch === '"') {
          if (text[i + 1] === '"') {
            field += '"';
            i++;
          } else {
            inQuotes = false;
          }
        } else {
          field += ch;
        }
      } else if (ch === '"') {
        inQuotes = true;
      } else if (ch === sep) {
        endField();
      } else if (ch === '\n') {
        endRow();
      } else if (ch !== '\r') {
        field += ch;
      }
    }
    if (field.trim() || row.length) endRow();
    return rows;
  }

  async function load() {
    loading = true;
    error = '';
    try {
      // Source view: show the artifact's code (the .jsx behind a compiled
      // .html when paired, otherwise the file's own text), shiki-highlighted.
      if (sourceView) {
        const srcUrl = codeSrc || src;
        const srcExt = (srcUrl.split('/').pop() || '').split('.').pop()?.toLowerCase() || '';
        const res = await fetch(srcUrl);
        if (!res.ok) throw new Error(loadFailed(res.status));
        const text = await res.text();
        const { codeToHtml } = await import('shiki');
        renderedHtml = await codeToHtml(text, {
          lang: CODE_LANGS[srcExt] || 'text',
          themes: { light: 'github-light', dark: 'github-dark' },
        });
        loading = false;
        return;
      }
      switch (mode) {
        case 'markdown': {
          const { meta, body } = splitFrontMatter(await fetchText());
          frontMatter = meta;
          renderedHtml = renderHtml(body);
          break;
        }
        case 'html':
          // Rendered via <iframe src> directly — no fetch needed. (srcdoc +
          // sandbox renders in Chromium but stays blank in Tauri's WKWebView;
          // a URL-loaded iframe works in both.)
          break;
        case 'code': {
          const text = await fetchText();
          const { codeToHtml } = await import('shiki');
          renderedHtml = await codeToHtml(text, {
            lang: CODE_LANGS[ext] || 'text',
            themes: { light: 'github-light', dark: 'github-dark' },
          });
          break;
        }
        case 'csv': {
          const text = await fetchText();
          const rows = parseCsv(text, ext === 'tsv' ? '\t' : ',');
          sheets = [{ name: title, rows: rows.slice(0, SHEET_ROW_CAP + 1), total: rows.length }];
          break;
        }
        case 'sheet':
          // SheetView fetches the engine's view model itself.
          break;
        case 'docx': {
          const data = await fetchBinary();
          const { renderAsync } = await import('docx-preview');
          loading = false; // container must render before pages attach
          await new Promise((r) => requestAnimationFrame(r));
          if (!docxContainer) return;
          docxContainer.replaceChildren();
          // Faithful paginated Word rendering: real pages with margins,
          // colors, shaded tables, headers/footers — not flattened HTML.
          await renderAsync(data, docxContainer, undefined, {
            inWrapper: true,
            ignoreWidth: false,
            ignoreHeight: false,
          });
          fitDocxPages();
          return;
        }
        case 'pdf': {
          await renderPdfFrom(await fetchBinary());
          return;
        }
        case 'pptx': {
          // Decks render through the PDF viewer via the server's on-demand
          // pptx→pdf preview (nebo-office). 503 = plugin missing → the error
          // branch offers the download instead.
          const res = await fetch(`${src}?preview=pdf`);
          if (!res.ok) {
            throw new Error(
              res.status === 503
                ? $t('chat.pptxPreviewNeedsPlugin')
                : $t('chat.failedToLoadPreview', { values: { status: res.status } })
            );
          }
          await renderPdfFrom(await res.arrayBuffer());
          return;
        }
        case 'image':
        case 'video':
        case 'download':
          break;
      }
      loading = false;
    } catch (e) {
      error = e instanceof Error ? e.message : $t('chat.failedToRender');
      loading = false;
    }
  }

  // Word pages render at true page size (e.g. 8.5in) — scale them down to the
  // panel width with CSS zoom (zoom keeps text crisp, unlike transform).
  function fitDocxPages() {
    if (!docxContainer) return;
    const pages = Array.from(docxContainer.querySelectorAll<HTMLElement>('section.docx'));
    if (!pages.length) return;
    pages.forEach((p) => { p.style.zoom = '1'; });
    const available = docxContainer.clientWidth;
    if (available <= 0) return;
    pages.forEach((p) => {
      const w = p.offsetWidth;
      if (w) p.style.zoom = String(Math.min(1, available / w));
    });
  }

  $effect(() => {
    if (!docxContainer) return;
    const ro = new ResizeObserver(() => fitDocxPages());
    ro.observe(docxContainer);
    return () => ro.disconnect();
  });

  // Shared PDF rendering for native PDFs and pptx previews.
  async function renderPdfFrom(data: ArrayBuffer) {
    const pdfjs = await import('pdfjs-dist');
    const workerUrl = (await import('pdfjs-dist/build/pdf.worker.min.mjs?url')).default;
    pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
    const doc = await pdfjs.getDocument({ data }).promise;
    loading = false; // container must render before canvases attach
    await renderPdfPages(pdfjs, doc);
  }

  async function renderPdfPages(_pdfjs: unknown, doc: { numPages: number; getPage: (n: number) => Promise<any> }) {
    // Wait a tick for the container to mount after `loading` flips.
    await new Promise((r) => requestAnimationFrame(r));
    if (!pdfContainer) return;
    pdfContainer.replaceChildren();
    for (let i = 1; i <= doc.numPages; i++) {
      const page = await doc.getPage(i);
      const viewport = page.getViewport({ scale: 1.4 });
      const canvas = document.createElement('canvas');
      canvas.width = viewport.width;
      canvas.height = viewport.height;
      canvas.className = 'w-full h-auto rounded-lg border border-base-300 mb-3';
      pdfContainer.appendChild(canvas);
      const ctx = canvas.getContext('2d');
      if (!ctx) continue;
      await page.render({ canvasContext: ctx, viewport, canvas }).promise;
    }
  }

  onMount(load);
</script>

<!-- html previews fill the panel edge-to-edge (the page scrolls inside the
     iframe); every other mode scrolls as padded content. -->
<div class={(mode === 'html' || mode === 'sheet') && !sourceView ? 'h-full' : 'p-4'}>
  {#if loading}
    <div class="text-xs text-base-content/50 py-8 text-center">{$t('common.loading')}</div>
  {:else if error}
    <div class="flex flex-col items-center gap-3 py-8">
      <div class="text-xs text-error">{error}</div>
      <a href={src} download={title} onclick={(e) => downloadArtifact(e, src, title)} class="btn btn-sm btn-outline">{$t('chat.downloadFile', { values: { title } })}</a>
    </div>
  {:else if sourceView}
    <div data-selectable class="text-xs leading-relaxed rounded-lg overflow-x-auto [&_pre]:p-4 [&_pre]:rounded-lg">{@html renderedHtml}</div>
  {:else if mode === 'markdown'}
    <!-- Documents read at a page's width, however wide the panel is. -->
    <div class="max-w-[52rem] mx-auto">
      {#if frontMatter.length}
        <div class="mb-5 rounded-lg bg-base-200/50 px-3.5 py-2.5 text-xs flex flex-col gap-1">
          {#each frontMatter as [key, value]}
            <div class="flex gap-2 min-w-0"><span class="text-base-content/50 shrink-0 w-24 truncate">{key}</span><span class="text-base-content/80 min-w-0">{value}</span></div>
          {/each}
        </div>
      {/if}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div data-selectable class="prose prose-sm max-w-none" onclick={oncontentclick}>{@html renderedHtml}</div>
    </div>
  {:else if mode === 'docx'}
    <div class="max-w-[52rem] mx-auto" bind:this={docxContainer}></div>
  {:else if mode === 'code'}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="text-xs leading-relaxed rounded-lg overflow-x-auto [&_pre]:p-4 [&_pre]:rounded-lg" onclick={oncontentclick}>{@html renderedHtml}</div>
  {:else if mode === 'html'}
    <!-- URL-loaded (srcdoc + sandbox stays blank in Tauri's WKWebView).
         Opaque origin: scripts may run but can't reach the app, API, or storage. -->
    <iframe
      sandbox="allow-scripts"
      src={src}
      title={title}
      class="w-full h-full border-0 bg-white"
    ></iframe>
  {:else if mode === 'pdf' || mode === 'pptx'}
    <div bind:this={pdfContainer}></div>
  {:else if mode === 'sheet'}
    <SheetView {documentId} {version} {src} {onsaved} {agentId} {sessionKey} />
  {:else if mode === 'csv'}
    {#each sheets as sheet}
      {#if sheets.length > 1}
        <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mt-4 mb-2 first:mt-0">{sheet.name}</div>
      {/if}
      <div class="overflow-x-auto scrollbar-slim rounded-lg border border-base-300 mb-2">
        <table class="table table-xs w-full">
          <thead>
            <tr class="bg-base-200">
              {#each sheet.rows[0] ?? [] as cell}
                <th class="text-xs font-semibold">{cell}</th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each sheet.rows.slice(1, SHEET_ROW_CAP + 1) as row}
              <tr class="border-t border-base-300">
                {#each row as cell}
                  <td class="text-xs">{cell}</td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if sheet.total > SHEET_ROW_CAP + 1}
        <div class="text-xs text-base-content/50 mb-3">{$t('chat.showingFirstRows', { values: { shown: SHEET_ROW_CAP, total: sheet.total - 1 } })}</div>
      {/if}
    {/each}
  {:else if mode === 'image'}
    <img src={src} alt={title} class="max-w-full h-auto rounded-lg border border-base-300" />
  {:else if mode === 'video'}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video src={firstFrame(src)} controls preload="metadata" class="max-w-full rounded-lg border border-base-300"></video>
  {:else if mode === 'audio'}
    <audio src={src} controls preload="metadata" class="w-full"></audio>
  {:else}
    <div class="flex flex-col items-center gap-3 py-10">
      <div class="text-sm font-medium">{title}</div>
      <div class="text-xs text-base-content/50 text-center max-w-[260px]">
        {$t('chat.noPreviewFormat')}
      </div>
      <a href={src} download={title} onclick={(e) => downloadArtifact(e, src, title)} class="btn btn-sm btn-primary">{$t('common.download')}</a>
    </div>
  {/if}
</div>
