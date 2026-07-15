<script lang="ts">
  /**
   * WorkViewer — the ONE renderer for Work-panel artifacts, routed by file
   * extension (Claude's renderer-matrix model). Heavy libraries (pdfjs-dist,
   * docx-preview, xlsx, shiki) load on demand via dynamic import so the main
   * bundle stays lean. Fetching lives here too: text formats fetch as text,
   * binary formats as ArrayBuffer, media not at all (the browser streams it).
   *
   * Security model (mirrors Claude's): HTML artifacts run in a sandboxed
   * iframe WITHOUT allow-same-origin (opaque origin — scripts may run but
   * can't reach the app, its API, or its storage). DOCX renders via
   * docx-preview to styled DOM (no scripts/macros execute). Spreadsheet
   * formulas are never evaluated — values only.
   */
  import { onMount } from 'svelte';
  import { downloadArtifact } from '$lib/chat/download';

  let {
    url,
    title,
    renderHtml,
    oncontentclick,
    sourceView = false,
    codeUrl,
  }: {
    url: string;
    title: string;
    /** Markdown → HTML renderer shared with the chat (mention chips + code-copy buttons). */
    renderHtml: (md: string) => string;
    oncontentclick?: (e: MouseEvent) => void;
    /** Show the artifact's source instead of its rendered form (Preview/Code toggle). */
    sourceView?: boolean;
    /** Source file behind a compiled artifact (the .jsx behind a .html). */
    codeUrl?: string;
  } = $props();

  const ext = $derived((title.split('.').pop() || '').toLowerCase());

  const IMAGE_EXTS = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg'];
  const VIDEO_EXTS = ['mp4', 'webm', 'mov'];
  const CODE_LANGS: Record<string, string> = {
    js: 'javascript', mjs: 'javascript', cjs: 'javascript', ts: 'typescript',
    py: 'python', rs: 'rust', go: 'go', json: 'json', sh: 'bash', bash: 'bash',
    css: 'css', yaml: 'yaml', yml: 'yaml', toml: 'toml', sql: 'sql',
    svelte: 'svelte', tsx: 'tsx', jsx: 'jsx', rb: 'ruby', java: 'java', html: 'html',
    c: 'c', h: 'c', cpp: 'cpp', xml: 'xml', md: 'markdown', markdown: 'markdown',
  };

  type Mode =
    | 'markdown' | 'html' | 'pdf' | 'sheet' | 'csv' | 'docx' | 'code'
    | 'pptx' | 'image' | 'video' | 'download';

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
    if (CODE_LANGS[ext]) return 'code';
    // ppt/pptx/doc and anything else: no faithful in-app preview — offer the file.
    return 'download';
  });

  let loading = $state(true);
  let error = $state('');
  /** Rendered HTML for markdown / docx / code modes. */
  let renderedHtml = $state('');
  /** Parsed sheet data: per sheet, name + rows. */
  let sheets = $state<{ name: string; rows: string[][]; total: number }[]>([]);
  let pdfContainer = $state<HTMLDivElement | null>(null);
  let docxContainer = $state<HTMLDivElement | null>(null);

  const SHEET_ROW_CAP = 500;

  async function fetchText(): Promise<string> {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`Failed to load (${res.status})`);
    return res.text();
  }

  async function fetchBinary(): Promise<ArrayBuffer> {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`Failed to load (${res.status})`);
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
        const srcUrl = codeUrl || url;
        const srcExt = (srcUrl.split('/').pop() || '').split('.').pop()?.toLowerCase() || '';
        const res = await fetch(srcUrl);
        if (!res.ok) throw new Error(`Failed to load (${res.status})`);
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
          renderedHtml = renderHtml(await fetchText());
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
        case 'sheet': {
          const data = await fetchBinary();
          const XLSX = await import('xlsx');
          const wb = XLSX.read(data, { type: 'array' });
          sheets = wb.SheetNames.map((name) => {
            const rows = XLSX.utils.sheet_to_json<string[]>(wb.Sheets[name], {
              header: 1, raw: false, defval: '',
            }) as unknown as string[][];
            return { name, rows: rows.slice(0, SHEET_ROW_CAP + 1), total: rows.length };
          });
          break;
        }
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
          const res = await fetch(`${url}?preview=pdf`);
          if (!res.ok) {
            throw new Error(
              res.status === 503
                ? 'Preview needs the nebo-office plugin — download to open in PowerPoint/Keynote.'
                : `Failed to load preview (${res.status})`
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
      error = e instanceof Error ? e.message : 'Failed to render this file.';
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
<div class={mode === 'html' && !sourceView ? 'h-full' : 'p-4'}>
  {#if loading}
    <div class="text-xs text-base-content/50 py-8 text-center">Loading…</div>
  {:else if error}
    <div class="flex flex-col items-center gap-3 py-8">
      <div class="text-xs text-error">{error}</div>
      <a href={url} download={title} onclick={(e) => downloadArtifact(e, url, title)} class="btn btn-sm btn-outline">Download {title}</a>
    </div>
  {:else if sourceView}
    <div data-selectable class="text-xs leading-relaxed rounded-lg overflow-x-auto [&_pre]:p-4 [&_pre]:rounded-lg">{@html renderedHtml}</div>
  {:else if mode === 'markdown'}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div data-selectable class="prose prose-sm max-w-none" onclick={oncontentclick}>{@html renderedHtml}</div>
  {:else if mode === 'docx'}
    <div bind:this={docxContainer}></div>
  {:else if mode === 'code'}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="text-xs leading-relaxed rounded-lg overflow-x-auto [&_pre]:p-4 [&_pre]:rounded-lg" onclick={oncontentclick}>{@html renderedHtml}</div>
  {:else if mode === 'html'}
    <!-- URL-loaded (srcdoc + sandbox stays blank in Tauri's WKWebView).
         Opaque origin: scripts may run but can't reach the app, API, or storage. -->
    <iframe
      sandbox="allow-scripts"
      src={url}
      title={title}
      class="w-full h-full border-0 bg-white"
    ></iframe>
  {:else if mode === 'pdf' || mode === 'pptx'}
    <div bind:this={pdfContainer}></div>
  {:else if mode === 'csv' || mode === 'sheet'}
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
        <div class="text-xs text-base-content/50 mb-3">Showing first {SHEET_ROW_CAP} of {sheet.total - 1} rows.</div>
      {/if}
    {/each}
  {:else if mode === 'image'}
    <img src={url} alt={title} class="max-w-full h-auto rounded-lg border border-base-300" />
  {:else if mode === 'video'}
    <!-- svelte-ignore a11y_media_has_caption -->
    <video src={url} controls class="max-w-full rounded-lg border border-base-300"></video>
  {:else}
    <div class="flex flex-col items-center gap-3 py-10">
      <div class="text-sm font-medium">{title}</div>
      <div class="text-xs text-base-content/50 text-center max-w-[260px]">
        No in-app preview for this format yet — download it to open in its native app.
      </div>
      <a href={url} download={title} onclick={(e) => downloadArtifact(e, url, title)} class="btn btn-sm btn-primary">Download</a>
    </div>
  {/if}
</div>
