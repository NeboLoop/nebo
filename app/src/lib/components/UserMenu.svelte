<script lang="ts">
  import { onMount } from 'svelte';
  import { auth } from '$lib/stores/auth.js';
  import UpdateBanner from '$lib/components/UpdateBanner.svelte';

  let displayName = $state('');
  let planName = $state('Free');

  let { collapsed = false } = $props();
  let open = $state(false);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [accountResp, subsResp] = await Promise.all([
        api.neboLoopAccountStatus().catch(() => null),
        api.neboLoopBillingSubscription().catch(() => null)
      ]);
      const account = accountResp as Record<string, unknown> | null;
      if (account?.displayName) {
        displayName = String(account.displayName);
      }
      const sub = subsResp as Record<string, unknown> | null;
      if (sub?.plan) {
        planName = String(sub.plan);
      }
    } catch {
      // Keep mock data
    }
  });

  function handleLogout() {
    auth.logout();
    open = false;
  }

  const menuItems = [
    { href: '/settings/account', label: 'Settings', icon: '⚙' },
    { href: '/settings/profile', label: 'Account', icon: '👤' },
    { href: '/settings/billing', label: 'Billing', icon: '💳' },
    { href: '/upgrade', label: 'Upgrade', icon: '↑' },
    null,
    { href: '/settings/about', label: 'About Nebo', icon: 'ℹ' },
    { href: '#logout', label: 'Log out', icon: '↪' },
  ];
</script>

<UpdateBanner {collapsed} />
<div class="relative border-t border-base-300 shrink-0">
  {#if open}
    <div class="fixed inset-0 z-40" onclick={() => open = false} role="presentation"></div>
    <div class="absolute bottom-full mb-1 bg-base-100 rounded-lg border border-base-300 shadow-lg py-1 z-50 {collapsed ? 'left-1 w-[160px]' : 'left-0 right-0 mx-1.5'}">
      {#each menuItems as item}
        {#if item === null}
          <div class="h-px bg-base-300 mx-2 my-1"></div>
        {:else if item.href === '#logout'}
          <button
            class="flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-base-200 transition-colors w-full text-left cursor-pointer border-none bg-transparent"
            onclick={handleLogout}
          >
            <span class="w-4 text-center text-sm">{item.icon}</span>
            {item.label}
          </button>
        {:else}
          <a
            href={item.href}
            class="flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-base-200 transition-colors"
            onclick={() => open = false}
          >
            <span class="w-4 text-center text-sm">{item.icon}</span>
            {item.label}
          </a>
        {/if}
      {/each}
    </div>
  {/if}

  <button
    class="w-full flex items-center cursor-pointer hover:bg-base-200 transition-colors bg-transparent border-none {collapsed ? 'justify-center py-2.5 px-0' : 'gap-2 py-2.5 px-3.5 text-left'}"
    onclick={() => open = !open}
  >
    <div class="w-7 h-7 rounded-full bg-primary text-primary-content flex items-center justify-center font-mono text-xs font-semibold shrink-0">{displayName.slice(0, 2).toUpperCase()}</div>
    {#if !collapsed}
      <div class="flex-1 min-w-0">
        <div class="text-sm font-medium truncate">{displayName}</div>
        <div class="text-xs text-base-content/70 truncate">{planName} Plan</div>
      </div>
      <span class="text-sm">&middot;&middot;&middot;</span>
    {/if}
  </button>
</div>
