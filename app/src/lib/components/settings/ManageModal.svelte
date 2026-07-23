<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { Snippet } from 'svelte';
  import X from 'lucide-svelte/icons/x';

  // Shared management modal chrome — the shell behind the plugin detail modal,
  // reused for skills and agents so configure/uninstall looks and works the same
  // everywhere. Body goes in `children`; trailing actions go in `footer`.
  let {
    title,
    subtitle = '',
    leading = '',
    onClose,
    children,
    footer,
  }: {
    title: string;
    subtitle?: string;
    leading?: string;
    onClose: () => void;
    children: Snippet;
    footer?: Snippet;
  } = $props();
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/40"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={(e) => { if (e.target === e.currentTarget) onClose(); }}
  onkeydown={(e) => { if (e.key === 'Escape') onClose(); }}
>
  <div class="bg-base-100 rounded-xl border border-base-300 shadow-xl w-full max-w-xl mx-4 max-h-[80vh] flex flex-col">
    <div class="flex items-center justify-between p-5 border-b border-base-content/10">
      <div class="flex items-center gap-3 min-w-0">
        {#if leading}
          <div class="w-10 h-10 rounded-lg bg-base-200 grid place-items-center text-lg font-semibold shrink-0">{leading}</div>
        {/if}
        <div class="min-w-0">
          <div class="text-base font-semibold truncate">{title}</div>
          {#if subtitle}
            <div class="text-xs text-base-content/50">{subtitle}</div>
          {/if}
        </div>
      </div>
      <button class="btn btn-ghost btn-sm btn-square" onclick={onClose} aria-label={$t('common.close')}>
        <X class="w-4 h-4" />
      </button>
    </div>

    <div class="p-5 overflow-y-auto flex-1 space-y-6">
      {@render children()}
    </div>

    {#if footer}
      <div class="flex items-center justify-between p-5 border-t border-base-content/10">
        {@render footer()}
      </div>
    {/if}
  </div>
</div>
