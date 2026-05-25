<!--
  Command Palette (Cmd+K)
  Global search overlay for quick navigation to pages, settings, and marketplace.
-->

<script lang="ts">
  import { goto } from '$app/navigation';
  import Search from 'lucide-svelte/icons/search';

  interface Props {
    show: boolean;
    onclose?: () => void;
  }

  let { show = $bindable(false), onclose }: Props = $props();

  let query = $state('');
  let selectedIndex = $state(0);
  let mouseActive = $state(false);

  interface PaletteItem {
    category: string;
    label: string;
    description?: string;
    icon: string;
    action: () => void;
  }

  const navItems: PaletteItem[] = [
    { category: 'Navigation', label: 'Agents', icon: 'cpu', action: () => goto('/') },
    { category: 'Navigation', label: 'Apps', icon: 'grid', action: () => goto('/apps') },
    { category: 'Navigation', label: 'Schedule', icon: 'calendar', action: () => goto('/schedule') },
    { category: 'Navigation', label: 'Marketplace', icon: 'store', action: () => goto('/marketplace') },
    { category: 'Navigation', label: 'Activity', icon: 'calendar', action: () => goto('/activity') },
    { category: 'Navigation', label: 'Events', icon: 'calendar', action: () => goto('/events') },
    { category: 'Navigation', label: 'Automations', icon: 'workflow', action: () => goto('/automate') },
  ];

  const settingsItems: PaletteItem[] = [
    { category: 'Settings', label: 'Account', icon: 'settings', action: () => goto('/settings/account') },
    { category: 'Settings', label: 'Profile & Theme', icon: 'settings', action: () => goto('/settings/profile') },
    { category: 'Settings', label: 'Billing', icon: 'settings', action: () => goto('/settings/billing') },
    { category: 'Settings', label: 'Usage', icon: 'settings', action: () => goto('/settings/usage') },
    { category: 'Settings', label: 'Identity', icon: 'settings', action: () => goto('/settings/identity') },
    { category: 'Settings', label: 'Personality', icon: 'settings', action: () => goto('/settings/personality') },
    { category: 'Settings', label: 'Rules', icon: 'settings', action: () => goto('/settings/rules') },
    { category: 'Settings', label: 'Advisors', icon: 'settings', action: () => goto('/settings/advisors') },
    { category: 'Settings', label: 'Permissions', icon: 'settings', action: () => goto('/settings/permissions') },
    { category: 'Settings', label: 'Sessions', icon: 'settings', action: () => goto('/settings/sessions') },
    { category: 'Settings', label: 'Memories', icon: 'settings', action: () => goto('/settings/memories') },
    { category: 'Settings', label: 'Developer', icon: 'settings', action: () => goto('/settings/developer') },
    { category: 'Settings', label: 'About', icon: 'settings', action: () => goto('/settings/about') },
  ];

  const marketplaceItems: PaletteItem[] = [
    { category: 'Marketplace', label: 'Browse Skills', icon: 'zap', action: () => goto('/marketplace/skills') },
    { category: 'Marketplace', label: 'Browse Agents', icon: 'cpu', action: () => goto('/marketplace/agents') },
    { category: 'Marketplace', label: 'Browse Plugins', icon: 'plug', action: () => goto('/marketplace/plugins') },
    { category: 'Marketplace', label: 'Browse Connectors', icon: 'plug', action: () => goto('/marketplace/connectors') },
    { category: 'Marketplace', label: 'Installed Items', icon: 'grid', action: () => goto('/marketplace/installed') },
  ];

  const allItems = [...navItems, ...settingsItems, ...marketplaceItems];

  const filteredItems = $derived.by(() => {
    const q = query.toLowerCase().trim();
    if (!q) return allItems;
    return allItems.filter(
      (item) =>
        item.label.toLowerCase().includes(q) ||
        (item.description && item.description.toLowerCase().includes(q))
    );
  });

  const flatFiltered = $derived(filteredItems.slice(0, 50));

  const groupedResults = $derived.by(() => {
    const items = flatFiltered;
    const groups: { category: string; items: PaletteItem[] }[] = [];
    for (const item of items) {
      const existing = groups.find((g) => g.category === item.category);
      if (existing) {
        existing.items.push(item);
      } else {
        groups.push({ category: item.category, items: [item] });
      }
    }
    return groups;
  });

  // Reset state when opened
  $effect(() => {
    if (show) {
      query = '';
      selectedIndex = 0;
      mouseActive = false;
      requestAnimationFrame(() => {
        const el = document.querySelector<HTMLInputElement>('.command-palette-card input');
        if (el) el.focus();
      });
    }
  });

  // Reset selection when query changes
  $effect(() => {
    query;
    selectedIndex = 0;
  });

  function handleKeydown(e: KeyboardEvent) {
    const items = flatFiltered;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      mouseActive = false;
      selectedIndex = items.length > 0 ? (selectedIndex + 1) % items.length : 0;
      scrollSelectedIntoView();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      mouseActive = false;
      selectedIndex = items.length > 0 ? (selectedIndex - 1 + items.length) % items.length : 0;
      scrollSelectedIntoView();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (items[selectedIndex]) {
        activate(items[selectedIndex]);
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  }

  function scrollSelectedIntoView() {
    requestAnimationFrame(() => {
      const el = document.querySelector('.command-palette-item[data-selected="true"]');
      if (el) el.scrollIntoView({ block: 'nearest' });
    });
  }

  function activate(item: PaletteItem) {
    close();
    item.action();
  }

  function close() {
    show = false;
    query = '';
    selectedIndex = 0;
    onclose?.();
  }

  function handleBackdropClick(e: MouseEvent) {
    if ((e.target as HTMLElement).classList.contains('command-palette-backdrop')) {
      close();
    }
  }

  const iconPaths: Record<string, string> = {
    grid: '<rect x="3" y="3" width="7" height="9" rx="1"/><rect x="14" y="3" width="7" height="5" rx="1"/><rect x="14" y="12" width="7" height="9" rx="1"/><rect x="3" y="16" width="7" height="5" rx="1"/>',
    cpu: '<path d="M12 8V4H8"/><rect x="8" y="8" width="8" height="8" rx="1"/><path d="M12 16v4h4"/><path d="M8 12H4"/><path d="M20 12h-4"/>',
    user: '<circle cx="12" cy="8" r="5"/><path d="M20 21a8 8 0 0 0-16 0"/>',
    workflow: '<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M6 21V9a9 9 0 0 0 9 9"/>',
    zap: '<path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/>',
    plug: '<path d="M12 2v4"/><path d="M12 18v4"/><path d="m4.93 4.93 2.83 2.83"/><path d="m16.24 16.24 2.83 2.83"/><path d="M2 12h4"/><path d="M18 12h4"/><path d="m4.93 19.07 2.83-2.83"/><path d="m16.24 7.76 2.83-2.83"/>',
    calendar: '<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/>',
    store: '<path d="m2 7 4.41-4.41A2 2 0 0 1 7.83 2h8.34a2 2 0 0 1 1.42.59L22 7"/><path d="M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8"/><path d="M15 22v-4a2 2 0 0 0-2-2h-2a2 2 0 0 0-2 2v4"/><rect width="20" height="5" x="2" y="7" rx="1"/>',
    settings: '<path d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"/><path d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"/>',
    plus: '<path d="M12 5v14"/><path d="M5 12h14"/>',
  };
</script>

{#if show}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="command-palette-backdrop" onclick={handleBackdropClick} onkeydown={handleKeydown}>
    <div class="command-palette-card">
      <div class="flex items-center gap-3 px-4 py-3.5 border-b border-base-content/10">
        <Search class="w-5 h-5 text-base-content/40 shrink-0" />
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          bind:value={query}
          placeholder="Search or run..."
          class="flex-1 bg-transparent border-none outline-none text-base text-base-content placeholder:text-base-content/40"
          autofocus
        />
        <kbd class="kbd kbd-xs">esc</kbd>
      </div>

      <div class="overflow-y-auto scrollbar-thin" style="max-height: 60vh;">
        {#each groupedResults as group}
          <div class="command-palette-category">{group.category}</div>
          {#each group.items as item}
            {@const globalIndex = flatFiltered.indexOf(item)}
            <button
              type="button"
              class="command-palette-item"
              data-selected={globalIndex === selectedIndex}
              onclick={() => activate(item)}
              onmousemove={() => { mouseActive = true; selectedIndex = globalIndex; }}
              onmouseenter={() => { if (mouseActive) selectedIndex = globalIndex; }}
            >
              <svg
                class="w-4 h-4 shrink-0 opacity-50"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
              >
                {@html iconPaths[item.icon] ?? ''}
              </svg>
              <span class="truncate">{item.label}</span>
              {#if item.description}
                <span class="text-xs text-base-content/50 truncate ml-auto">{item.description}</span>
              {/if}
            </button>
          {/each}
        {/each}

        {#if groupedResults.length === 0}
          <div class="px-4 py-8 text-center text-base text-base-content/60">
            No results for "{query}"
          </div>
        {/if}
      </div>

      <div class="command-palette-footer">
        <span><kbd class="kbd kbd-xs">&uarr;&darr;</kbd> navigate</span>
        <span><kbd class="kbd kbd-xs">&crarr;</kbd> select</span>
        <span><kbd class="kbd kbd-xs">esc</kbd> close</span>
      </div>
    </div>
  </div>
{/if}
