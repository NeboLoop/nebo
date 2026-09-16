<script lang="ts">
  import { t } from 'svelte-i18n';
  import { formatRelative } from '$lib/time';
  import Loader from 'lucide-svelte/icons/loader';
  import type { LayerSeat } from '$lib/api/nebo';

  // Each employee's progress through the change it was told to read.
  //
  // `written` — it read the change and wrote its own section.
  // `pending` — its read is running now.
  // `stale`   — its last read did not finish. NOT an error: it is still working
  //             from what it wrote before, so it is never shown as a failure.
  let { seats }: { seats: LayerSeat[] } = $props();

  const DOT: Record<string, string> = {
    written: 'bg-success',
    pending: 'bg-warning',
    stale: 'bg-base-content/25',
  };
  const CHIP: Record<string, string> = {
    written: 'bg-success/10 text-success',
    pending: 'bg-warning/10 text-warning',
    stale: 'bg-base-200 text-base-content/60',
  };

  const KNOWN = ['written', 'pending', 'stale'];
  const staleCount = $derived(seats.filter((s) => s.status === 'stale').length);

  function statusLabel(status: string): string {
    return KNOWN.includes(status) ? $t('settingsLayers.seatStatus.' + status) : status;
  }
</script>

<div class="flex flex-col gap-1.5">
  {#each seats as seat (seat.id)}
    <div class="flex items-center gap-3 py-2 px-3 rounded-lg border border-base-300 bg-base-100">
      <span class="w-2 h-2 rounded-full shrink-0 {DOT[seat.status] ?? 'bg-base-content/25'}"></span>
      <div class="flex-1 min-w-0">
        <div class="text-sm font-medium truncate">{seat.name}</div>
        {#if seat.against}
          <div class="text-xs font-mono text-base-content/50 truncate">
            {$t('settingsLayers.readAgainst', { values: { stamp: seat.against } })}
          </div>
        {/if}
      </div>
      {#if seat.at}
        <span class="text-xs text-base-content/40 shrink-0">{formatRelative(seat.at)}</span>
      {/if}
      <span class="shrink-0 flex items-center gap-1 px-1.5 py-0.5 rounded text-xs font-medium {CHIP[seat.status] ?? CHIP.stale}">
        {#if seat.status === 'pending'}<Loader class="w-3 h-3 animate-spin" />{/if}
        {statusLabel(seat.status)}
      </span>
    </div>
  {/each}
</div>

{#if staleCount > 0}
  <p class="text-xs text-base-content/60 mt-2">{$t('settingsLayers.staleExplainer')}</p>
{/if}
