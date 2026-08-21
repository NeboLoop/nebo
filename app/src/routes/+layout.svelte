<script lang="ts">
  import '../app.css';
  import '$lib/i18n';
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { goto, appPath } from '$lib/nav';
  import { computerOpen } from '$lib/stores/computer';
  import DesktopView from '$lib/components/chat/DesktopView.svelte';
  import { beforeNavigate } from '$app/navigation';
  import { base } from '$app/paths';
  import { mobileAgentsOpen } from '$lib/stores/mobileNav';
  import { storage } from '$lib/storage';
  import { onMount } from 'svelte';
  import BrandMark from '$lib/components/BrandMark.svelte';

  // Under the tunnel base (/t/<botID>/), goto() is base-aware via $lib/nav but
  // raw <a href="/x"> links would still escape the prefix onto the hub's site.
  // Catch any same-origin navigation that leaves the base and re-root it.
  beforeNavigate((nav) => {
    if (!base || !nav.to) return;
    const to = nav.to.url;
    if (to.origin !== location.origin || to.pathname.startsWith(base)) return;
    nav.cancel();
    goto(to.pathname + to.search + to.hash);
  });
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { theme } from '$lib/stores/theme.js';
  import { onboardingComplete, onboardingChecked, backendReady, backendChecking, checkOnboardingStatus, retryBackendConnection } from '$lib/stores/onboarding';
  import Toast from '$lib/components/Toast.svelte';
  import { unreadCount, loadNotifications } from '$lib/stores/notifications';
  import CommandPalette from '$lib/components/CommandPalette.svelte';
  import UpgradeSuccessModal from '$lib/components/UpgradeSuccessModal.svelte';
  import OnboardingTour from '$lib/components/onboarding/OnboardingTour.svelte';
  import InstallFlowModal from '$lib/components/install/InstallFlowModal.svelte';
  import ApprovalGate from '$lib/components/ApprovalGate.svelte';
  let { children } = $props();

  let showCommandPalette = $state(false);
  let showUpgradeSuccess = $state(false);
  let upgradedPlan = $state('');

  // Show the upgrade-success modal when the plan changes (idiomatic WS subscription).
  onWsEvent<{ plan?: string }>('plan_changed', (d) => {
    if (d?.plan) {
      upgradedPlan = d.plan;
      showUpgradeSuccess = true;
    }
  });

  // Check onboarding status and initialize WebSocket on mount
  onMount(() => {
    checkOnboardingStatus();
    // Load notifications once so the Inbox badge is populated app-wide (idempotent).
    loadNotifications();

    // Expose global for the Tauri tray + app menu "Check for Updates" items
    (window as any).__NEBO_CHECK_UPDATE__ = async () => {
      const { checkForUpdates } = await import('$lib/stores/update');
      await checkForUpdates();
    };

    // Connect WebSocket once onboarding is done, then attach event listeners
    const unsub = onboardingComplete.subscribe(complete => {
      if (complete) {
        import('$lib/websocket/client').then(({ getWebSocketClient }) => {
          const ws = getWebSocketClient();
          if (!ws.isConnected()) {
            const token = storage.get('nebo_token');
            ws.connect(token || undefined);
          }
          // Attach real-time event listeners after connecting
          import('$lib/websocket/listeners').then(({ attachWebSocketListeners }) => {
            attachWebSocketListeners();
          });
        });
      }
    });

    return () => {
      unsub();
      delete (window as any).__NEBO_CHECK_UPDATE__;
      import('$lib/websocket/listeners').then(({ detachWebSocketListeners }) => {
        detachWebSocketListeners();
      });
    };
  });

  // Redirect to onboarding if not complete (wait for check to finish)
  $effect(() => {
    if ($onboardingChecked && !$onboardingComplete && !appPath($page.url.pathname).startsWith('/onboarding')) {
      goto('/onboarding');
    }
  });

  // Block browser right-click except in selectable areas and inputs
  function handleContextMenu(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (
      target.closest('[data-selectable]') ||
      target.closest('[data-context-menu]') ||
      target.closest('textarea') ||
      target.closest('input') ||
      target.closest('pre') ||
      target.closest('code')
    ) return;
    e.preventDefault();
  }

  function handleGlobalKeydown(e: KeyboardEvent) {
    // Electron/Claude-Desktop-style reload: ⌘R (mac) / Ctrl-R (win/linux).
    // Tauri release builds strip dev reload shortcuts, so wire it explicitly.
    if ((e.metaKey || e.ctrlKey) && (e.key === 'r' || e.key === 'R')) {
      e.preventDefault();
      window.location.reload();
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
      e.preventDefault();
      showCommandPalette = !showCommandPalette;
    } else if (e.key === 'Escape' && showCommandPalette) {
      e.preventDefault();
      showCommandPalette = false;
    }
  }

  const sections = [
    { id: 'agents', path: '/', label: 'nav.agents' },
    { id: 'inbox', path: '/inbox', label: 'nav.inbox' },
    { id: 'schedule', path: '/schedule', label: 'nav.schedule' },
    { id: 'marketplace', path: '/marketplace', label: 'nav.marketplace' },
  ];

  const activeSection = $derived.by(() => {
    const p = appPath($page.url.pathname);
    if (p === '/') return 'agents';
    for (const s of sections) {
      if (s.path !== '/' && p.startsWith(s.path)) return s.id;
    }
    // Agent deep links like /researcher, /ops — match if route has agentId param
    if ($page.params.agentId) return 'agents';
    return '';
  });

  const isEmbed = $derived(appPath($page.url.pathname).startsWith('/chat-embed'));

  const isMinimalChrome = $derived(
    isEmbed ||
    appPath($page.url.pathname).startsWith('/settings') ||
    appPath($page.url.pathname).startsWith('/app/')
  );
</script>

<svelte:window onkeydown={handleGlobalKeydown} oncontextmenu={handleContextMenu} />

{#if isEmbed}
  {@render children()}
{:else if !$backendReady && !$onboardingChecked}
  <div class="h-dvh flex flex-col items-center justify-center bg-base-100 gap-4">
    <BrandMark class="w-12 h-12" />
    {#if $backendChecking}
      <span class="loading loading-spinner loading-md"></span>
      <p class="text-sm text-base-content/70">{$t('layout.connectingToNebo')}</p>
    {:else}
      <p class="text-sm text-base-content/70">{$t('layout.waitingForBackend')}</p>
      <span class="loading loading-dots loading-sm"></span>
    {/if}
    <button
      class="btn btn-sm btn-outline mt-2"
      disabled={$backendChecking}
      onclick={() => retryBackendConnection()}
    >
      {$t('layout.retryNow')}
    </button>
  </div>
{:else if !$onboardingChecked}
  <div class="h-dvh flex items-center justify-center bg-base-100">
    <span class="loading loading-spinner loading-lg"></span>
  </div>
{:else if appPath($page.url.pathname).startsWith('/onboarding')}
  {@render children()}
{:else if !$onboardingComplete}
  <!-- Check finished, onboarding not complete, and we're not on /onboarding yet:
       the redirect in the $effect above is in flight. Show the splash instead of
       letting the main app paint for a frame (otherwise the UI flashes the app and
       then jumps to onboarding). -->
  <div class="h-dvh flex items-center justify-center bg-base-100">
    <span class="loading loading-spinner loading-lg"></span>
  </div>
{:else}
  <div class="flex flex-col h-dvh">
    {#if !isMinimalChrome}
      <header class="h-14 border-b border-base-300 bg-base-100 flex items-center px-2 md:px-4 shrink-0">
        {#if activeSection === 'agents'}
          <!-- Mobile: opens the Employees drawer (the sidebar is a slide-over below md) -->
          <button
            class="md:hidden w-9 h-9 mr-1 rounded-md flex items-center justify-center border-none bg-transparent cursor-pointer text-base-content/70"
            aria-label="Employees"
            onclick={() => mobileAgentsOpen.update((v) => !v)}
          >
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>
          </button>
        {/if}
        <a href="/" class="hidden md:flex items-center gap-1.5 font-semibold text-sm tracking-tight mr-2 md:mr-4">
          <BrandMark class="w-6 h-6" />
          <span class="hidden md:inline">Nebo</span>
        </a>
        <!-- eager code preload: over the tunnel a lazy route chunk costs a full
             round-trip on first tap; fetch all nav chunks up front instead -->
        <nav class="flex items-center h-full gap-0.5 md:gap-1 min-w-0 overflow-x-auto" data-sveltekit-preload-code="eager">
          {#each sections as s}
            <a
              href={s.path}
              data-tour={s.id}
              class="px-2 md:px-3 h-full flex items-center gap-1.5 text-sm font-medium border-t-3 border-t-transparent border-b-3 transition-colors whitespace-nowrap shrink-0 {activeSection === s.id
                ? 'border-primary text-base-content'
                : 'border-transparent text-base-content/70 hover:text-base-content'}"
            >
              {$t(s.label)}
              {#if s.id === 'inbox' && $unreadCount > 0}
                <span class="badge badge-error badge-xs text-error-content font-semibold">{$unreadCount > 9 ? '9+' : $unreadCount}</span>
              {/if}
            </a>
          {/each}
        </nav>
        <div class="flex-1"></div>
        <!-- The bot's computer — installation-wide (one desktop per Nebo,
             shared by every employee), so it lives up here, not in a chat. -->
        <button
          onclick={() => computerOpen.update((v) => !v)}
          title={$t('chat.openComputer')}
          class="flex items-center justify-center h-8 w-8 mr-1 md:mr-2 rounded-field cursor-pointer border border-base-300 bg-base-100 text-base-content/70 hover:text-base-content {$computerOpen ? 'text-primary border-primary/40' : ''}"
        >
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="3" width="20" height="14" rx="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>
        </button>
        <button
          onclick={() => (showCommandPalette = true)}
          data-tour="search"
          class="hidden md:flex items-center h-8 w-auto md:w-48 rounded-field px-2 md:px-3 gap-1.5 text-sm cursor-pointer border border-base-300 bg-base-100"
        >
          <span class="font-mono text-sm py-px px-1 rounded-sm bg-base-200">&#x2318;K</span>
          <span class="text-base-content/70">{$t('nav.searchOrRun')}</span>
        </button>
      </header>
    {/if}
    <div class="flex-1 flex min-h-0">
      {@render children()}
    </div>
    <OnboardingTour />
  </div>
{/if}
{#if !isEmbed}
  <Toast />
  <!-- The ONE install/configure modal for the whole app. Opened via the
       installFlow store (product/configure) or window nebo:code_* events (code
       paste). Mounted once here so two install modals can never stack. -->
  <InstallFlowModal />
  <!-- The ONE tool-approval modal. Driven by the `approval_request` WS event
       (runner pauses an OFF-capability tool call). Mounted once here so it shows
       over any view; sends the decision back via `approval_response`. -->
  <ApprovalGate />
  <CommandPalette bind:show={showCommandPalette} />
  <!-- The ONE computer drawer: the installation's shared desktop. -->
  {#if $computerOpen}
    <div class="fixed inset-0 z-40 flex justify-end">
      <button
        type="button"
        class="absolute inset-0 bg-black/30 border-none cursor-default"
        aria-label={$t('chat.closeComputer')}
        onclick={() => computerOpen.set(false)}
      ></button>
      <div class="relative h-full w-full md:w-[70%] lg:w-[60%] bg-base-100 shadow-xl flex flex-col">
        <DesktopView onclose={() => computerOpen.set(false)} />
      </div>
    </div>
  {/if}
  <UpgradeSuccessModal
    bind:show={showUpgradeSuccess}
    plan={upgradedPlan}
    onclose={() => {
      showUpgradeSuccess = false;
      if (appPath($page.url.pathname).startsWith('/pricing')) goto('/');
    }}
  />
{/if}
