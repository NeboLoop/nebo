<script lang="ts">
  import { t } from 'svelte-i18n';
  import X from 'lucide-svelte/icons/x';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import PrettyJson from './PrettyJson.svelte';
  import type { Notification } from '$lib/stores/notifications.js';

  let { notif, onClose, onAction }: {
    notif: Notification;
    onClose: () => void;
    onAction: (n: Notification) => void;
  } = $props();

  const typeColors: Record<string, string> = {
    agent: 'bg-success',
    system: 'bg-info',
    warning: 'bg-warning',
    error: 'bg-error',
  };

  // Render the body as a structured tree when it is JSON, otherwise as wrapped text.
  const parsed = $derived.by((): unknown => {
    const b = notif.message?.trim();
    if (!b || (b[0] !== '{' && b[0] !== '[')) return null;
    try {
      const p = JSON.parse(b);
      return p && typeof p === 'object' ? p : null;
    } catch { return null; }
  });
</script>

<div class="fixed inset-0 z-[60] flex items-center justify-center p-4">
  <div class="absolute inset-0 bg-black/40" onclick={onClose} role="presentation"></div>
  <div class="relative w-full max-w-lg max-h-[80vh] flex flex-col bg-base-100 rounded-xl border border-base-300 shadow-xl overflow-hidden">
    <!-- Header -->
    <div class="flex items-start gap-3 px-5 py-4 border-b border-base-content/10">
      <div class="w-2 h-2 rounded-full mt-2 shrink-0 {typeColors[notif.type] || 'bg-info'}"></div>
      <div class="flex-1 min-w-0">
        <div class="text-base font-semibold break-words">{notif.title}</div>
        <div class="text-xs text-base-content/50 font-mono mt-0.5">{notif.time}</div>
      </div>
      <button
        onclick={onClose}
        class="p-1 rounded hover:bg-base-content/10 transition-colors cursor-pointer bg-transparent border-none shrink-0"
        aria-label={$t('common.close')}
      >
        <X class="w-4 h-4 text-base-content/50" />
      </button>
    </div>

    <!-- Body: pretty by default -->
    <div class="px-5 py-4 overflow-y-auto">
      {#if parsed}
        <div class="p-3 rounded-lg border border-base-300 bg-base-200/30 overflow-x-auto">
          <PrettyJson value={parsed} />
        </div>
      {:else if notif.message}
        <p class="text-sm text-base-content/80 whitespace-pre-wrap break-words">{notif.message}</p>
      {:else}
        <p class="text-sm text-base-content/40">{$t('notifications.noDetail')}</p>
      {/if}
    </div>

    <!-- Action -->
    {#if notif.link}
      <div class="flex justify-end gap-2 px-5 py-3 border-t border-base-content/10">
        <button
          onclick={() => onAction(notif)}
          class="btn btn-primary btn-sm gap-1"
        >
          {$t('notifications.takeAction')} <ArrowRight class="w-3.5 h-3.5" />
        </button>
      </div>
    {/if}
  </div>
</div>
