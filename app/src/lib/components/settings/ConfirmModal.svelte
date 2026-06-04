<script lang="ts">
  // Shared confirmation dialog. Stacks above ManageModal (z-[60] > z-50).
  let {
    title,
    message,
    confirmLabel = 'Confirm',
    onConfirm,
    onCancel,
    busy = false,
  }: {
    title: string;
    message: string;
    confirmLabel?: string;
    onConfirm: () => void;
    onCancel: () => void;
    busy?: boolean;
  } = $props();
</script>

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_noninteractive_element_interactions -->
<div
  class="fixed inset-0 z-[60] flex items-center justify-center bg-black/40"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={(e) => { if (e.target === e.currentTarget) onCancel(); }}
  onkeydown={(e) => { if (e.key === 'Escape') onCancel(); }}
>
  <div class="bg-base-100 rounded-xl border border-base-300 shadow-xl w-full max-w-sm mx-4">
    <div class="p-5">
      <div class="text-base font-semibold mb-1">{title}</div>
      <p class="text-xs text-base-content/70">{message}</p>
    </div>
    <div class="flex items-center justify-end gap-2 p-4 border-t border-base-content/10">
      <button
        class="px-3 py-1.5 rounded-md border border-base-300 text-xs cursor-pointer bg-transparent hover:bg-base-200 transition-colors"
        onclick={onCancel}
        disabled={busy}
      >Cancel</button>
      <button
        class="px-3 py-1.5 rounded-md border border-error/30 text-xs text-error font-medium cursor-pointer bg-transparent hover:bg-error/5 transition-colors"
        onclick={onConfirm}
        disabled={busy}
      >{busy ? 'Removing…' : confirmLabel}</button>
    </div>
  </div>
</div>
