<!--
  The "over" tier: things you go and do and come back from — Inbox,
  Marketplace, Settings, Runs. The workspace stays visible behind it: you have
  not left your conversation, you are standing in front of it.

  Desktop: a centered card. Phone: a bottom-sheet DRAWER — slides up, grab
  handle, drag down to dismiss — because that is the native idiom, and a
  full-screen white takeover reads as "you left the app".

  Open state belongs in the URL at the call site, so every one of these is
  deep-linkable and the back button behaves.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from 'svelte-i18n';
  import X from 'lucide-svelte/icons/x';

  interface Props {
    open: boolean;
    title: string;
    onclose: () => void;
    /** Employee identity chip before the title — same avatar/color as the roster row. */
    avatarInitial?: string;
    avatarClass?: string;
    /** Compact card for single-object editors (a reminder, a field group). */
    narrow?: boolean;
    /** Header content between the title and the close button. */
    actions?: Snippet;
    children: Snippet;
  }

  let { open, title, onclose, avatarInitial = '', avatarClass = '', narrow = false, actions, children }: Props = $props();

  function onkeydown(e: KeyboardEvent) {
    // defaultPrevented = a stacked shelf already consumed this Escape —
    // one press closes one layer, never the whole stack.
    if (e.key === 'Escape' && open && !e.defaultPrevented) {
      e.preventDefault();
      onclose();
    }
  }

  // ── Drag-to-dismiss (touch only; the handle is hidden on md+).
  // Live translate while dragging, dismiss past the threshold, spring back
  // otherwise. Only downward drags move the sheet.
  let sheet = $state<HTMLDivElement | null>(null);
  let dragStartY: number | null = null;
  let dragDy = $state(0);
  let dragging = $state(false);
  const DISMISS_AT = 110;

  function dragStart(e: PointerEvent) {
    // The whole header is the drag surface, but its buttons stay buttons.
    if ((e.target as HTMLElement).closest('button')) return;
    dragStartY = e.clientY;
    dragging = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function dragMove(e: PointerEvent) {
    if (dragStartY === null) return;
    dragDy = Math.max(0, e.clientY - dragStartY);
  }
  function dragEnd() {
    if (dragStartY === null) return;
    const shouldClose = dragDy > DISMISS_AT;
    dragStartY = null;
    dragging = false;
    dragDy = 0;
    if (shouldClose) onclose();
  }
</script>

<svelte:window {onkeydown} />

{#if open}
  <div class="fixed inset-0 z-[70] flex items-center justify-center max-md:items-end max-md:p-0 p-4 md:p-8">
    <div
      class="absolute inset-0 bg-black/40"
      onclick={onclose}
      role="presentation"
    ></div>
    <div
      bind:this={sheet}
      class="relative flex flex-col {narrow ? 'w-[min(92vw,30rem)] h-auto max-h-[min(90vh,40rem)] max-md:h-auto max-md:max-h-[93dvh]' : 'w-[min(96vw,72rem)] h-[min(90vh,48rem)] max-md:h-[93dvh]'} rounded-2xl border border-base-300 max-md:w-full max-md:rounded-b-none max-md:border-0 max-md:pb-[env(safe-area-inset-bottom)] max-md:shadow-[0_-8px_30px_rgba(0,0,0,0.18)] bg-base-100 shadow-2xl overflow-hidden {dragging ? '' : 'max-md:transition-transform max-md:duration-200'} motion-safe:max-md:animate-[sheet-up_0.24s_ease-out]"
      style={dragDy > 0 ? `transform: translateY(${dragDy}px)` : ''}
      role="dialog"
      aria-modal="true"
      aria-label={title}
    >
      <!-- Grab surface: the handle AND the whole header row drag the sheet
           on touch (a 20px pill alone is an unhittable target); header
           buttons opt out inside dragStart. -->
      <div
        class="max-md:touch-none max-md:cursor-grab shrink-0"
        onpointerdown={dragStart}
        onpointermove={dragMove}
        onpointerup={dragEnd}
        onpointercancel={dragEnd}
        role="presentation"
      >
        <div class="md:hidden pt-3 pb-2 flex justify-center">
          <div class="w-12 h-1.5 rounded-full bg-base-content/25"></div>
        </div>

        <div class="h-12 max-md:h-10 px-4 border-b border-base-300 flex items-center gap-2">
        {#if avatarInitial}
          <span class="w-6 h-6 rounded-md flex items-center justify-center font-mono text-[10px] font-semibold shrink-0 {avatarClass}">{avatarInitial}</span>
        {/if}
        <span class="text-sm font-semibold">{title}</span>
        <div class="flex-1"></div>
        {#if actions}{@render actions()}{/if}
        <button
          type="button"
          class="w-7 h-7 max-md:w-9 max-md:h-9 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none"
          onclick={onclose}
          title={$t('common.close')}
        >
          <X class="w-4 h-4" />
        </button>
        </div>
      </div>
      <div class="flex-1 min-h-0 flex">
        {@render children()}
      </div>
    </div>
  </div>
{/if}

<style>
  @keyframes -global-sheet-up {
    from { transform: translateY(24%); opacity: 0.6; }
    to { transform: translateY(0); opacity: 1; }
  }
</style>
