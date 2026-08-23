<!--
  The one collapsible left rail. Owns the three things every column needs and
  every column used to reimplement: persisted collapse state, the rail↔expanded
  swap, and the off-canvas drawer below md.

  Collapse is per-section and persisted (stores/sidebar.ts). Below md there is no
  rail state — collapsing a full-screen overlay to 48px means nothing — so the
  toggle closes the drawer instead.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sidebarCollapsedFor } from '$lib/stores/sidebar';

  interface Props {
    /** Key for persisted collapse state, e.g. 'workspace' | 'marketplace'. */
    section: string;
    /** Shown in the header when expanded. */
    title?: string;
    /** Drawer open below md — plain state down, event up. The caller owns
     *  where that state lives (for the workspace it is the URL). Omit both
     *  for a rail with no mobile drawer. */
    mobileOpen?: boolean;
    onmobileclose?: () => void;
    /** Rendered at the far left of the header, in both states. */
    leading?: Snippet;
    /** Extra header content, right of the title, left of the toggle. */
    headerActions?: Snippet;
    /** Body when expanded (and always, on mobile). */
    expanded: Snippet;
    /** Body at 48px. Omit to render nothing when collapsed. */
    collapsed?: Snippet;
    /** Pinned to the bottom in both states. */
    footer?: Snippet<[boolean]>;
    tour?: string;
  }

  let {
    section,
    title,
    mobileOpen,
    onmobileclose,
    leading,
    headerActions,
    expanded,
    collapsed,
    footer,
    tour
  }: Props = $props();

  const isCollapsed = sidebarCollapsedFor(section);
  const drawerOpen = $derived(mobileOpen ?? false);
  const hasDrawer = $derived(mobileOpen !== undefined);

  // On mobile the drawer is full-width, so the rail rendering never applies.
  const showRail = $derived($isCollapsed && !drawerOpen);

  function toggle() {
    if (typeof window !== 'undefined' && !window.matchMedia('(min-width: 768px)').matches) {
      onmobileclose?.();
      return;
    }
    isCollapsed.set(!$isCollapsed);
  }
</script>

{#if drawerOpen}
  <div
    class="fixed inset-0 z-30 bg-black/40 md:hidden"
    onclick={() => onmobileclose?.()}
    role="presentation"
  ></div>
{/if}

<div
  data-tour={tour}
  class="{$isCollapsed
    ? 'md:w-rail-collapsed md:min-w-rail-collapsed'
    : 'md:w-rail md:min-w-rail'} {hasDrawer
    ? `max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:w-full max-md:transition-[transform,visibility] ${
        drawerOpen ? 'max-md:translate-x-0 max-md:shadow-2xl' : 'max-md:-translate-x-full max-md:invisible'
      }`
    : ''} border-r border-base-300 shadow-[2px_0_8px_-2px_rgba(0,0,0,0.08)] flex flex-col bg-base-200 shrink-0 transition-all duration-150"
>
  <div
    class="h-11 border-b border-base-300 flex items-center gap-2 shrink-0 {showRail
      ? 'justify-center'
      : 'px-3.5 justify-between'}"
  >
    {#if leading && !showRail}{@render leading()}{/if}
    {#if !showRail && title}
      <span class="text-sm font-semibold flex-1 truncate">{title}</span>
    {/if}
    {#if !showRail && headerActions}
      {@render headerActions()}
    {/if}
    <button
      class="w-7 h-7 max-md:w-10 max-md:h-10 rounded-md flex items-center justify-center hover:bg-base-100 cursor-pointer bg-transparent border-none shrink-0"
      onclick={toggle}
      title={$isCollapsed ? $t('nav.expandSidebar') : $t('nav.collapseSidebar')}
    >
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none">
        <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.2" />
        <line x1="5.5" y1="3" x2="5.5" y2="13" stroke="currentColor" stroke-width="1.2" />
      </svg>
    </button>
  </div>

  <div class="flex-1 overflow-y-auto overflow-x-hidden min-h-0">
    {#if showRail}
      {#if collapsed}{@render collapsed()}{/if}
    {:else}
      {@render expanded()}
    {/if}
  </div>

  {#if footer}
    <div class="shrink-0 max-md:pb-[env(safe-area-inset-bottom)]">
      {@render footer(showRail)}
    </div>
  {/if}
</div>
