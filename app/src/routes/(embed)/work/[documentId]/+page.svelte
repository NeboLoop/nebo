<script lang="ts">
  /**
   * Standalone work-document viewer — the page the web Library (and any deep
   * link) opens. Same ONE renderer as the chat's Work panel (WorkViewer),
   * served shell-less by the (embed) layout so it reads like a document, not
   * an app. Through the tunnel it lives at /t/<botID>/work/<documentId> and
   * inherits the owner's session; on desktop it's the same page on localhost.
   */
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { t } from 'svelte-i18n';
  import WorkViewer from '$lib/components/chat/WorkViewer.svelte';
  import { backendBase, backendUrl } from '$lib/api/base';
  import { parseMarkdown } from '$lib/markdown';
  import { downloadArtifact } from '$lib/chat/download';

  interface DocListing {
    id: string;
    filename: string;
    kind: string;
    latestVersion: number;
    url: string;
    chatTitle?: string | null;
  }

  let doc = $state<DocListing | null>(null);
  let failed = $state(false);
  let viewSource = $state(false);

  const docId = $derived($page.params.documentId ?? '');
  const canToggleSource = $derived(
    !!doc && ['html', 'md', 'markdown', 'txt'].includes((doc.filename.split('.').pop() || '').toLowerCase())
  );

  onMount(async () => {
    try {
      const resp = await fetch(`${backendBase()}/api/v1/work/documents?id=${encodeURIComponent(docId)}`);
      const body = await resp.json();
      doc = body?.documents?.[0] ?? null;
      failed = !doc;
    } catch {
      failed = true;
    }
  });
</script>

<svelte:head>
  <title>{doc?.filename ?? 'Document'} | Nebo</title>
</svelte:head>

<div class="h-dvh flex flex-col bg-base-100">
  <header class="flex items-center gap-2 px-4 py-2.5 border-b border-base-300 shrink-0">
    <span class="text-sm font-semibold truncate flex-1" title={doc?.chatTitle ?? undefined}>
      {doc?.filename ?? '…'}
      {#if doc && doc.latestVersion > 1}
        <span class="ml-1.5 text-xs font-normal text-base-content/50">{$t('chat.versionN', { values: { version: doc.latestVersion } })}</span>
      {/if}
    </span>
    {#if canToggleSource}
      <div class="flex items-center rounded-md bg-base-200 p-0.5 shrink-0">
        <button
          class="py-0.5 px-2 rounded text-xs cursor-pointer border-none transition-colors {!viewSource ? 'bg-base-100 font-medium shadow-sm' : 'bg-transparent text-base-content/60 hover:text-base-content'}"
          onclick={() => (viewSource = false)}
        >{$t('chat.preview')}</button>
        <button
          class="py-0.5 px-2 rounded text-xs cursor-pointer border-none transition-colors {viewSource ? 'bg-base-100 font-medium shadow-sm' : 'bg-transparent text-base-content/60 hover:text-base-content'}"
          onclick={() => (viewSource = true)}
        >{$t('chat.artifactCode')}</button>
      </div>
    {/if}
    {#if doc}
      <a
        href={backendUrl(doc.url)}
        download={doc.filename}
        onclick={(e) => downloadArtifact(e, doc?.url ?? '', doc?.filename)}
        class="py-1 px-2.5 rounded-md text-xs font-medium bg-base-200 hover:bg-base-300 text-base-content/80 hover:text-base-content transition-colors shrink-0 no-underline"
      >{$t('common.download')}</a>
    {/if}
  </header>

  <main class="flex-1 min-h-0 overflow-y-auto">
    {#if doc}
      {#key `${doc.id}:${viewSource}`}
        <WorkViewer url={doc.url} title={doc.filename} renderHtml={parseMarkdown} sourceView={viewSource} />
      {/key}
    {:else if failed}
      <div class="h-full flex items-center justify-center text-sm text-base-content/60">
        {$t('chat.documentNotFound')}
      </div>
    {:else}
      <div class="h-full flex items-center justify-center">
        <span class="loading loading-spinner loading-md"></span>
      </div>
    {/if}
  </main>
</div>
