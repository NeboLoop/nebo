<!--
  The "over" tier: a large card above the workspace for things you go and do
  and come back from — Inbox, Marketplace, employee settings. The workspace
  stays visible behind it, which is the point: you have not left your
  conversation, you are standing in front of it.

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
    /** Header content between the title and the close button. */
    actions?: Snippet;
    children: Snippet;
  }

  let { open, title, onclose, actions, children }: Props = $props();

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape' && open) {
      e.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window {onkeydown} />

{#if open}
  <div class="fixed inset-0 z-[70] flex items-center justify-center max-md:p-0 p-4 md:p-8">
    <div
      class="absolute inset-0 bg-black/40"
      onclick={onclose}
      role="presentation"
    ></div>
    <div
      class="relative flex flex-col w-[min(96vw,72rem)] h-[min(90vh,48rem)] max-md:w-full max-md:h-full rounded-2xl max-md:rounded-none max-md:pb-[env(safe-area-inset-bottom)] bg-base-100 border border-base-300 max-md:border-0 shadow-2xl overflow-hidden"
      role="dialog"
      aria-modal="true"
      aria-label={title}
    >
      <div class="h-12 px-4 border-b border-base-300 flex items-center gap-2 shrink-0">
        <span class="text-sm font-semibold">{title}</span>
        <div class="flex-1"></div>
        {#if actions}{@render actions()}{/if}
        <button
          type="button"
          class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none"
          onclick={onclose}
          title={$t('common.close')}
        >
          <X class="w-4 h-4" />
        </button>
      </div>
      <div class="flex-1 min-h-0 flex">
        {@render children()}
      </div>
    </div>
  </div>
{/if}
