<!--
  AppListingCard — an app's marketplace listing in the chat (App Developer
  mode). The listing tools' results carry it; the hub's review outcome
  (`app_listing` event) updates the status in place while the chat is open.
-->
<script lang="ts" module>
  export interface AppListingPayload {
    kind: 'app_listing';
    appId?: string;
    artifactId?: string;
    name?: string;
    shortDescription?: string;
    version?: string;
    visibility?: string;
    category?: string | null;
    status?: string;
    notes?: string;
    screenshots?: { fileId?: string; path?: string; label?: string }[];
    [k: string]: unknown;
  }
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { backendUrl } from '$lib/api/base';

  let { listing }: { listing: AppListingPayload } = $props();

  type Outcome = { appId?: string; artifactId?: string; status?: string; notes?: string; version?: string };
  let live = $state<Outcome | null>(null);
  onWsEvent<Outcome>('app_listing', (data) => {
    if (data?.artifactId && data.artifactId === listing.artifactId) live = data;
  });

  const status = $derived(live?.status ?? listing.status ?? 'draft');
  const notes = $derived(live?.notes ?? listing.notes ?? '');
  const version = $derived(live?.version ?? listing.version ?? '');
  const statusKey = $derived(
    status === 'approved'
      ? 'agent.listingApproved'
      : status === 'rejected'
        ? 'agent.listingRejected'
        : status === 'in_review' || status === 'submitted'
          ? 'agent.listingInReview'
          : 'agent.listingDraft',
  );
  const badge = $derived(
    status === 'approved' ? 'badge-success' : status === 'rejected' ? 'badge-warning' : status === 'draft' ? 'badge-ghost' : 'badge-info',
  );
  const shots = $derived((listing.screenshots ?? []).filter((s) => s.path));

  function shotUrl(path: string): string {
    return backendUrl('/api/v1/files/' + path.split('/').map(encodeURIComponent).join('/'));
  }
</script>

<div class="my-2.5 rounded-box border border-base-300 bg-base-100 p-3 max-w-xl">
  <div class="flex items-center gap-2 min-w-0">
    <span class="font-semibold text-sm truncate">{listing.name}</span>
    {#if version}<span class="text-xs text-base-content/60 shrink-0">v{version}</span>{/if}
    <span class="badge badge-sm {badge} ml-auto shrink-0">{$t(statusKey)}</span>
  </div>
  {#if listing.shortDescription}
    <p class="text-sm text-base-content/80 mt-1">{listing.shortDescription}</p>
  {/if}
  {#if shots.length}
    <div class="flex gap-2 mt-2 overflow-x-auto">
      {#each shots as shot (shot.fileId ?? shot.path)}
        <img
          src={shotUrl(shot.path ?? '')}
          alt={shot.label ?? ''}
          class="h-28 w-auto rounded-lg border border-base-content/15 object-cover shrink-0"
          loading="lazy"
        />
      {/each}
    </div>
  {/if}
  {#if notes && status === 'rejected'}
    <p class="text-xs text-base-content/70 mt-2"><span class="font-medium">{$t('agent.listingNotes')}:</span> {notes}</p>
  {/if}
</div>
