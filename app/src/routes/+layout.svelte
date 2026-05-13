<script lang="ts">
  import '../app.css';
  import '$lib/i18n';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { theme } from '$lib/stores/theme.js';
  import { onboardingComplete, onboardingChecked, backendReady, backendChecking, checkOnboardingStatus, retryBackendConnection } from '$lib/stores/onboarding';
  import Toast from '$lib/components/Toast.svelte';
  import NotificationBell from '$lib/components/NotificationBell.svelte';
  import CommandPalette from '$lib/components/CommandPalette.svelte';
  import UpgradeSuccessModal from '$lib/components/UpgradeSuccessModal.svelte';
  let { children } = $props();

  let showCommandPalette = $state(false);
  let showUpgradeSuccess = $state(false);
  let upgradedPlan = $state('');

  // Check onboarding status and initialize WebSocket on mount
  onMount(() => {
    checkOnboardingStatus();

    // Connect WebSocket once onboarding is done, then attach event listeners
    const unsub = onboardingComplete.subscribe(complete => {
      if (complete) {
        import('$lib/websocket/client').then(({ getWebSocketClient }) => {
          const ws = getWebSocketClient();
          if (!ws.isConnected()) {
            const token = typeof localStorage !== 'undefined' ? localStorage.getItem('nebo_token') : null;
            ws.connect(token || undefined);
          }
          // Attach real-time event listeners after connecting
          import('$lib/websocket/listeners').then(({ attachWebSocketListeners }) => {
            attachWebSocketListeners();
          });
        });
      }
    });

    // Listen for plan_changed events to show upgrade success modal
    const handlePlanChanged = (e: Event) => {
      const detail = (e as CustomEvent).detail;
      if (detail?.plan) {
        upgradedPlan = detail.plan;
        showUpgradeSuccess = true;
      }
    };
    window.addEventListener('nebo:plan_changed', handlePlanChanged);

    return () => {
      unsub();
      window.removeEventListener('nebo:plan_changed', handlePlanChanged);
      import('$lib/websocket/listeners').then(({ detachWebSocketListeners }) => {
        detachWebSocketListeners();
      });
    };
  });

  // Redirect to onboarding if not complete (wait for check to finish)
  $effect(() => {
    if ($onboardingChecked && !$onboardingComplete && !$page.url.pathname.startsWith('/onboarding')) {
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
    if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
      e.preventDefault();
      showCommandPalette = !showCommandPalette;
    } else if (e.key === 'Escape' && showCommandPalette) {
      e.preventDefault();
      showCommandPalette = false;
    }
  }

  const sections = [
    { id: 'agents', path: '/', label: 'Agents' },
    { id: 'schedule', path: '/schedule', label: 'Schedule' },
    { id: 'apps', path: '/apps', label: 'Apps' },
    { id: 'marketplace', path: '/marketplace', label: 'Marketplace' },
  ];

  const activeSection = $derived.by(() => {
    const p = $page.url.pathname;
    if (p === '/') return 'agents';
    for (const s of sections) {
      if (s.path !== '/' && p.startsWith(s.path)) return s.id;
    }
    // Agent deep links like /researcher, /ops — match if route has agentId param
    if ($page.params.agentId) return 'agents';
    return '';
  });

  const isEmbed = $derived($page.url.pathname.startsWith('/chat-embed'));

  const isMinimalChrome = $derived(
    isEmbed ||
    $page.url.pathname.startsWith('/settings') ||
    $page.url.pathname.startsWith('/app/')
  );
</script>

<svelte:window onkeydown={handleGlobalKeydown} oncontextmenu={handleContextMenu} />

{#if isEmbed}
  {@render children()}
{:else if !$backendReady && !$onboardingChecked}
  <div class="h-dvh flex flex-col items-center justify-center bg-base-100 gap-4">
    <div class="w-10 h-10 rounded-lg bg-primary text-primary-content flex items-center justify-center font-mono text-xl font-bold">N</div>
    {#if $backendChecking}
      <span class="loading loading-spinner loading-md"></span>
      <p class="text-sm text-base-content/70">Connecting to Nebo...</p>
    {:else}
      <p class="text-sm text-base-content/70">Waiting for backend...</p>
      <span class="loading loading-dots loading-sm"></span>
    {/if}
    <button
      class="btn btn-sm btn-outline mt-2"
      disabled={$backendChecking}
      onclick={() => retryBackendConnection()}
    >
      Retry now
    </button>
  </div>
{:else if !$onboardingChecked}
  <div class="h-dvh flex items-center justify-center bg-base-100">
    <span class="loading loading-spinner loading-lg"></span>
  </div>
{:else if $page.url.pathname.startsWith('/onboarding')}
  {@render children()}
{:else}
  <div class="flex flex-col h-screen">
    {#if !isMinimalChrome}
      <header class="h-14 border-b border-base-300 bg-base-100 flex items-center px-4 shrink-0">
        <a href="/" class="flex items-center gap-1.5 font-semibold text-sm tracking-tight mr-4">
          <div class="w-5 h-5 rounded bg-primary text-primary-content flex items-center justify-center font-mono text-sm font-bold">N</div>
          Nebo
        </a>
        <nav class="flex items-center h-full gap-1">
          {#each sections as s}
            <a
              href={s.path}
              class="px-3 h-full flex items-center text-sm font-medium border-b-3 transition-colors {activeSection === s.id
                ? 'border-primary text-base-content'
                : 'border-transparent text-base-content/70 hover:text-base-content'}"
            >{s.label}</a>
          {/each}
        </nav>
        <div class="flex-1"></div>
        <button
          onclick={() => (showCommandPalette = true)}
          class="flex items-center h-8 w-48 rounded-field px-3 gap-1.5 text-sm cursor-pointer border border-base-300 bg-base-100"
        >
          <span class="font-mono text-sm py-px px-1 rounded-sm bg-base-200">&#x2318;K</span>
          <span class="text-base-content/70">Search or run...</span>
        </button>
        <NotificationBell />
      </header>
    {/if}
    <div class="flex-1 flex min-h-0">
      {@render children()}
    </div>
  </div>
{/if}
{#if !isEmbed}
  <Toast />
  <CommandPalette bind:show={showCommandPalette} />
  <UpgradeSuccessModal
    bind:show={showUpgradeSuccess}
    plan={upgradedPlan}
    onclose={() => {
      showUpgradeSuccess = false;
      if ($page.url.pathname.startsWith('/upgrade')) goto('/');
    }}
  />
{/if}
