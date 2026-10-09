<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount } from 'svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import UpdateBanner from '$lib/components/UpdateBanner.svelte';
  import FeedbackModal from '$lib/components/FeedbackModal.svelte';

  let displayName = $state('');
  let planName = $state('');

  let { collapsed = false } = $props();
  let open = $state(false);
  let feedbackOpen = $state(false);

  onMount(async () => {
    try {
      const api = await import('$lib/api/nebo');
      const [accountResp, subsResp] = await Promise.all([
        api.neboAIAccountStatus().catch(() => null),
        api.neboAIBillingSubscription().catch(() => null)
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

  // A plan bought (or ended) while the app is open: the hub's tokenRefresh
  // reaches the server, which announces it here.
  onWsEvent<{ plan?: string }>('plan_changed', (d) => {
    if (d?.plan) planName = d.plan;
  });

  const menuItems = [
    { href: '/settings/account', label: 'nav.botSettings', icon: '🏢' },
    { href: '/settings/account', label: 'settings.navItems.account', icon: '👤' },
    { href: '/settings/billing', label: 'settings.navItems.billing', icon: '💳' },
    null,
    { action: 'feedback', label: 'userMenu.provideFeedback', icon: '✉' },
    { href: '/settings/about', label: 'userMenu.aboutNebo', icon: 'ℹ' },
  ];
</script>

<UpdateBanner {collapsed} />
<div class="relative border-t border-base-300 shrink-0">
  {#if open}
    <div class="fixed inset-0 z-40" onclick={() => open = false} role="presentation"></div>
    <div class="absolute bottom-full mb-1 bg-base-100 rounded-lg border border-base-300 shadow-lg py-1 z-50 {collapsed ? 'start-1 w-[160px]' : 'start-0 end-0 mx-1.5'}">
      {#each menuItems as item}
        {#if item === null}
          <div class="h-px bg-base-300 mx-2 my-1"></div>
        {:else if 'action' in item}
          <button
            type="button"
            class="w-full flex items-center gap-2 px-3 py-1.5 text-sm text-start hover:bg-base-200 transition-colors bg-transparent border-none cursor-pointer"
            onclick={() => { open = false; feedbackOpen = true; }}
          >
            <span class="w-4 text-center text-sm" aria-hidden="true">{item.icon}</span>
            {$t(item.label)}
          </button>
        {:else}
          <a
            href={item.href}
            class="flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-base-200 transition-colors"
            onclick={() => open = false}
          >
            <span class="w-4 text-center text-sm">{item.icon}</span>
            {$t(item.label)}
          </a>
        {/if}
      {/each}
    </div>
  {/if}

  <button
    class="w-full flex items-center cursor-pointer hover:bg-base-200 transition-colors bg-transparent border-none {collapsed ? 'justify-center py-2.5 px-0' : 'gap-2 py-2.5 px-3.5 text-start'}"
    onclick={() => open = !open}
  >
    <div class="w-7 h-7 rounded-full bg-primary text-primary-content flex items-center justify-center font-mono text-xs font-semibold shrink-0">{displayName.slice(0, 2).toUpperCase()}</div>
    {#if !collapsed}
      <div class="flex-1 min-w-0">
        <div class="text-sm font-medium truncate">{displayName}</div>
        <div class="text-xs text-base-content/70 truncate">{$t('userMenu.planSuffix', { values: { plan: planName || $t('common.free') } })}</div>
      </div>
      <span class="text-sm">&middot;&middot;&middot;</span>
    {/if}
  </button>
</div>

<FeedbackModal open={feedbackOpen} onclose={() => (feedbackOpen = false)} />
