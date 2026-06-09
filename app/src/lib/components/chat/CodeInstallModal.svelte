<!--
  CodeInstallModal — shows install progress when user pastes a marketplace code in chat.
  Phases: installing → confirm → processing → checkout → auth → done | error
  Driven entirely by WebSocket events from the backend.
-->

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import { approveDeps, authLogin, neboAIBillingPaymentMethods } from '$lib/api/nebo';
  import { createMarketplaceSubscription } from '$lib/api/index';
  import type { PaymentMethodInfo } from '$lib/api/neboComponents';

  type Phase = 'installing' | 'confirm' | 'processing' | 'checkout' | 'auth' | 'done' | 'error';

  type DepItem = {
    reference: string;
    type: string;
    name?: string;
    artifactId?: string;
    status: 'pending' | 'installing' | 'installed' | 'failed';
    error?: string;
  };

  type TierInfo = {
    name?: string;
    recurringPriceCents?: number;
    billingInterval?: string;
    pricingModel?: string;
  };

  let {
    show = $bindable(false),
    onclose,
    onAgentSetup,
  }: {
    show: boolean;
    onclose?: () => void;
    onAgentSetup?: (agentId: string, agentName: string) => void;
  } = $props();

  let phase = $state<Phase>('installing');
  let code = $state('');
  let codeType = $state('');
  let artifactName = $state('');
  let artifactId = $state('');
  let artifactType = $state('');
  let statusMessage = $state('');
  let errorMessage = $state('');
  let checkoutUrl = $state('');
  let deps = $state<DepItem[]>([]);
  // Total top-level deps the backend announced (dep_cascade_start) — lets the
  // progress bar be determinate. 0 until announced; falls back to deps.length.
  let depTotal = $state(0);
  // Reference of the code most recently copied, for transient "Copied" feedback.
  let copiedRef = $state('');
  // Installed items that still need credentials/config before they work.
  let needsSetup = $state<Array<{ slug: string; label: string; description: string }>>([]);
  let authLabel = $state('');
  let authDescription = $state('');
  let authInProgress = $state(false);
  let pluginSlug = $state('');
  let authQueue = $state<Array<{ slug: string; label: string; description: string }>>([]);
  let authIndex = $state(0);
  let pendingAgentId = $state('');
  // True when the user kicked off the install from the desktop UI: the modal
  // stays open until they dismiss it. False for channel/loop-triggered installs
  // (no human waiting), which auto-dismiss. Defaults true (safer: never flash away).
  let interactive = $state(true);

  // Purchase confirmation state
  let tier = $state<TierInfo | null>(null);
  let paymentMethod = $state<PaymentMethodInfo | null>(null);
  let paymentMethodLoading = $state(false);
  let confirmLoading = $state(false);
  let stripeCheckout = $state<any>(null);
  let stripeContainerEl = $state<HTMLDivElement | undefined>(undefined);

  const typeLabel = $derived(codeType ? codeType.charAt(0).toUpperCase() + codeType.slice(1) : 'Code');
  const title = $derived(
    phase === 'done'
      ? `${typeLabel} Installed`
      : phase === 'error'
        ? 'Install Failed'
        : phase === 'confirm'
          ? 'Confirm Purchase'
          : phase === 'processing'
            ? 'Processing Payment'
            : phase === 'checkout'
              ? 'Complete Payment'
              : phase === 'auth'
                ? `Connect ${authLabel || 'Account'}`
                : `Installing ${typeLabel}`
  );
  const installedCount = $derived(deps.filter((d) => d.status === 'installed').length);
  const failedCount = $derived(deps.filter((d) => d.status === 'failed').length);
  const settledCount = $derived(installedCount + failedCount);
  // Determinate progress denominator: the announced total, or what we've seen.
  const progressTotal = $derived(Math.max(depTotal, deps.length));
  const progressPct = $derived(progressTotal > 0 ? Math.round((settledCount / progressTotal) * 100) : 0);
  const installing = $derived(deps.some((d) => d.status === 'installing'));

  function formatPrice(cents: number, interval?: string): string {
    const amount = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'usd', minimumFractionDigits: 0 }).format(cents / 100);
    if (interval === 'year') return `${amount}/yr`;
    if (interval === 'month') return `${amount}/mo`;
    return amount;
  }

  function reset() {
    phase = 'installing';
    code = '';
    codeType = '';
    artifactName = '';
    artifactId = '';
    artifactType = '';
    statusMessage = '';
    errorMessage = '';
    checkoutUrl = '';
    deps = [];
    depTotal = 0;
    needsSetup = [];
    authLabel = '';
    authDescription = '';
    authInProgress = false;
    pluginSlug = '';
    authQueue = [];
    authIndex = 0;
    pendingAgentId = '';
    interactive = true;
    tier = null;
    paymentMethod = null;
    paymentMethodLoading = false;
    confirmLoading = false;
    if (stripeCheckout) { stripeCheckout.destroy(); stripeCheckout = null; }
  }

  function findOrAddDep(reference: string, type: string, name?: string, artifactId?: string): number {
    const idx = deps.findIndex((d) => d.reference === reference);
    if (idx >= 0) {
      // Backfill name/id if a later event carries them.
      if ((name && !deps[idx].name) || (artifactId && !deps[idx].artifactId)) {
        deps[idx] = { ...deps[idx], name: deps[idx].name ?? name, artifactId: deps[idx].artifactId ?? artifactId };
      }
      return idx;
    }
    deps = [...deps, { reference, type, name, artifactId, status: 'pending' }];
    return deps.length - 1;
  }

  let installTimeout: ReturnType<typeof setTimeout> | null = null;

  /** Notify sidebar to refresh agent roster after an install completes */
  function notifySidebarRefresh() {
    if (typeof window !== 'undefined') {
      window.dispatchEvent(new CustomEvent('nebo:agent_installed', { detail: {} }));
    }
  }

  function close() {
    if (installTimeout) { clearTimeout(installTimeout); installTimeout = null; }
    if (stripeCheckout) { stripeCheckout.destroy(); stripeCheckout = null; }
    show = false;
    onclose?.();
  }

  /** Auto-close only for channel/loop-triggered installs; interactive ones wait for the user. */
  function autoCloseIfRemote(delay = 1500) {
    if (!interactive) setTimeout(close, delay);
  }

  // --- WS event handlers ---

  function handleCodeProcessing(e: Event) {
    const data = (e as CustomEvent).detail;
    reset();
    code = (data?.code as string) || '';
    codeType = (data?.code_type as string) || '';
    statusMessage = (data?.status_message as string) || 'Processing...';
    interactive = data?.interactive !== false;
    show = true;

    // Safety net: if no code_result arrives within 30s, show soft completion
    if (installTimeout) clearTimeout(installTimeout);
    installTimeout = setTimeout(() => {
      if (phase === 'installing') {
        statusMessage = `${typeLabel} installed — finalizing dependencies...`;
        phase = 'done';
        notifySidebarRefresh();
        autoCloseIfRemote(2000);
      }
    }, 30_000);
  }

  function handleCodeResult(e: Event) {
    if (installTimeout) { clearTimeout(installTimeout); installTimeout = null; }
    const data = (e as CustomEvent).detail;
    const success = data?.success as boolean;
    const paymentRequired = data?.payment_required as boolean;
    const checkout = data?.checkout_url as string | undefined;
    const name = (data?.artifact_name as string) || '';
    const agentId = (data?.artifact_id as string) || '';
    const aType = (data?.artifact_type as string) || '';
    const error = (data?.error as string) || '';
    const message = (data?.message as string) || '';
    const tierData = data?.tier as TierInfo | undefined;

    if (name) artifactName = name;
    if (agentId) artifactId = agentId;
    if (aType) artifactType = aType;

    if (codeType === 'agent' && agentId) {
      pendingAgentId = agentId;
    }

    if (paymentRequired) {
      checkoutUrl = checkout || '';
      if (tierData) tier = tierData;
      // Fetch payment methods, then show confirm phase
      fetchPaymentMethodAndConfirm();
    } else if (success) {
      // If we were waiting for payment completion, this is the post-payment install
      if (phase === 'processing' || phase === 'checkout') {
        notifySidebarRefresh();
        statusMessage = message || `${artifactName || typeLabel} installed`;
        phase = 'done';
        autoCloseIfRemote();
        return;
      }
      if (phase === 'auth') return;
      notifySidebarRefresh();
      // Only hand off to agent setup when auth is required
      const needsAuth = data?.needsAuth as boolean;
      if (codeType === 'agent' && agentId && needsAuth && onAgentSetup) {
        show = false;
        onAgentSetup(agentId, artifactName);
        return;
      }
      statusMessage = message || `${artifactName || typeLabel} installed`;
      phase = 'done';
      autoCloseIfRemote();
    } else {
      errorMessage = error || 'Installation failed';
      phase = 'error';
    }
  }

  async function fetchPaymentMethodAndConfirm() {
    paymentMethodLoading = true;
    phase = 'confirm';
    try {
      const resp = await neboAIBillingPaymentMethods();
      const methods = (resp as any)?.methods as PaymentMethodInfo[] | undefined;
      paymentMethod = methods?.find((m: PaymentMethodInfo) => m.isDefault) || methods?.[0] || null;
    } catch {
      paymentMethod = null;
    } finally {
      paymentMethodLoading = false;
    }
  }

  async function confirmPurchase() {
    confirmLoading = true;
    try {
      // Create marketplace subscription — NeboAI returns a checkout URL
      const resp = await createMarketplaceSubscription({
        targetId: artifactId,
        targetType: artifactType || codeType,
        botCount: 1,
      });

      if (resp.checkoutUrl) {
        // Open checkout in system browser — backend handles redirect checkout
        window.open(resp.checkoutUrl, '_blank');
        phase = 'processing';
        statusMessage = 'Complete payment in your browser...';
        // Set timeout: if no code_result arrives in 5 minutes, allow close
        installTimeout = setTimeout(() => {
          if (phase === 'processing') {
            statusMessage = 'Payment processing...';
          }
        }, 300_000);
      } else {
        // Subscription created directly (free tier fallback)
        phase = 'processing';
        statusMessage = 'Finalizing...';
      }
    } catch (e: any) {
      errorMessage = e?.message || 'Failed to start checkout';
      phase = 'error';
    } finally {
      confirmLoading = false;
    }
  }

  function handlePluginInstalling(e: Event) {
    const data = (e as CustomEvent).detail;
    const plugin = (data?.plugin as string) || '';
    if (!plugin) return;
    const idx = findOrAddDep(plugin, 'plugin');
    deps[idx] = { ...deps[idx], status: 'installing' };
    deps = deps;
  }

  function handlePluginInstalled(e: Event) {
    const data = (e as CustomEvent).detail;
    const plugin = (data?.plugin as string) || '';
    if (!plugin) return;
    const idx = findOrAddDep(plugin, 'plugin');
    deps[idx] = { ...deps[idx], status: 'installed' };
    deps = deps;
  }

  /** Total announced before the cascade starts — drives the determinate bar.
   *  Only trusted during the live install (ignored on retry from the done view). */
  function handleDepCascadeStart(e: Event) {
    if (phase !== 'installing') return;
    const total = Number((e as CustomEvent).detail?.total ?? 0);
    if (total > 0) depTotal = total;
  }

  function handleDepNeedsSetup(e: Event) {
    const items = ((e as CustomEvent).detail?.items as typeof needsSetup) || [];
    if (Array.isArray(items) && items.length > 0) needsSetup = items;
  }

  /** Open the canonical plugin config UI (Settings → Plugins), then close. */
  function openPluginSettings() {
    close();
    goto('/settings/plugins');
  }

  function handleDepStarted(e: Event) {
    const data = (e as CustomEvent).detail;
    const reference = (data?.reference as string) || '';
    const depType = ((data?.depType as string) || 'skill').toLowerCase();
    const name = (data?.name as string) || undefined;
    const artifactId = (data?.artifactId as string) || undefined;
    if (!reference) return;
    const idx = findOrAddDep(reference, depType, name, artifactId);
    // Don't downgrade a settled row if events arrive out of order.
    if (deps[idx].status === 'pending') {
      deps[idx] = { ...deps[idx], status: 'installing' };
      deps = deps;
    }
  }

  function handleDepPending(e: Event) {
    const data = (e as CustomEvent).detail;
    const reference = (data?.reference as string) || '';
    const depType = ((data?.depType as string) || 'skill').toLowerCase();
    const name = (data?.name as string) || undefined;
    const artifactId = (data?.artifactId as string) || undefined;
    if (reference) findOrAddDep(reference, depType, name, artifactId);
  }

  function handleDepInstalled(e: Event) {
    const data = (e as CustomEvent).detail;
    const reference = (data?.reference as string) || '';
    const depType = ((data?.depType as string) || 'skill').toLowerCase();
    const name = (data?.name as string) || undefined;
    const artifactId = (data?.artifactId as string) || undefined;
    if (!reference) return;
    const idx = findOrAddDep(reference, depType, name, artifactId);
    deps[idx] = { ...deps[idx], status: 'installed', error: undefined };
    deps = deps;
  }

  function handleDepFailed(e: Event) {
    const data = (e as CustomEvent).detail;
    const reference = (data?.reference as string) || '';
    const depType = ((data?.depType as string) || 'skill').toLowerCase();
    const name = (data?.name as string) || undefined;
    const artifactId = (data?.artifactId as string) || undefined;
    const error = (data?.error as string) || 'Unknown error';
    if (!reference) return;
    const idx = findOrAddDep(reference, depType, name, artifactId);
    deps[idx] = { ...deps[idx], status: 'failed', error };
    deps = deps;
  }

  /** Retry a single failed dependency in place via the cascade-approve endpoint.
   *  The row flips to a spinner; dep_installed/dep_failed events settle it.
   *  The artifact id lets the backend recognise an already-installed plugin. */
  async function retryDep(dep: DepItem) {
    const idx = deps.findIndex((d) => d.reference === dep.reference);
    if (idx < 0) return;
    deps[idx] = { ...deps[idx], status: 'installing', error: undefined };
    deps = deps;
    try {
      // DepType deserializes as snake_case — dep.type is already lowercase.
      await approveDeps({
        deps: [{ depType: dep.type, reference: dep.reference, name: dep.name, artifactId: dep.artifactId }],
      });
    } catch {
      deps[idx] = { ...deps[idx], status: 'failed', error: 'Retry failed to start' };
      deps = deps;
    }
  }

  function handleAgentAuthRequired(e: Event) {
    const data = (e as CustomEvent).detail;
    const plugins = (data?.plugins as Array<{ slug: string; label: string; description: string }>) || [];
    if (plugins.length === 0) return;
    authQueue = plugins;
    authIndex = 0;
    pluginSlug = plugins[0].slug;
    authLabel = plugins[0].label;
    authDescription = plugins[0].description || '';
    phase = 'auth';
  }

  function handlePluginAuthUrl(e: Event) {
    const data = (e as CustomEvent).detail;
    const url = data?.url as string;
    if (url && show && phase === 'auth') {
      window.open(url, '_blank');
    }
  }

  function handlePluginAuthComplete(_e: Event) {
    if (!show || phase !== 'auth') return;
    authInProgress = false;

    if (authQueue.length > 0) {
      authIndex++;
      if (authIndex < authQueue.length) {
        const next = authQueue[authIndex];
        pluginSlug = next.slug;
        authLabel = next.label;
        authDescription = next.description || '';
        return;
      }
      if (pendingAgentId && onAgentSetup) {
        show = false;
        onAgentSetup(pendingAgentId, artifactName);
        return;
      }
    }

    phase = 'done';
    autoCloseIfRemote();
  }

  function handlePluginAuthError(e: Event) {
    if (!show || phase !== 'auth') return;
    const data = (e as CustomEvent).detail;
    authInProgress = false;
    errorMessage = (data?.error as string) || 'Authentication failed';
    phase = 'error';
  }

  async function startAuth() {
    if (!pluginSlug) return;
    authInProgress = true;
    try {
      await authLogin(pluginSlug);
    } catch {
      authInProgress = false;
      errorMessage = 'Failed to start authentication';
      phase = 'error';
    }
  }

  function skipAuth() {
    if (authQueue.length > 0) {
      authIndex++;
      if (authIndex < authQueue.length) {
        const next = authQueue[authIndex];
        pluginSlug = next.slug;
        authLabel = next.label;
        authDescription = next.description || '';
        return;
      }
      if (pendingAgentId && onAgentSetup) {
        show = false;
        onAgentSetup(pendingAgentId, artifactName);
        return;
      }
    }
    phase = 'done';
    autoCloseIfRemote();
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') close();
  }

  let copyTimeout: ReturnType<typeof setTimeout> | null = null;
  async function copyCode(reference: string) {
    try {
      await navigator.clipboard.writeText(reference);
      copiedRef = reference;
      if (copyTimeout) clearTimeout(copyTimeout);
      copyTimeout = setTimeout(() => { copiedRef = ''; }, 1500);
    } catch {
      // Clipboard unavailable (e.g. insecure context) — nothing actionable.
    }
  }

  // Subscribe to WS-dispatched DOM events
  onMount(() => {
    window.addEventListener('nebo:code_processing', handleCodeProcessing);
    window.addEventListener('nebo:code_result', handleCodeResult);
    window.addEventListener('nebo:plugin_installing', handlePluginInstalling);
    window.addEventListener('nebo:plugin_installed', handlePluginInstalled);
    window.addEventListener('nebo:dep_cascade_start', handleDepCascadeStart);
    window.addEventListener('nebo:dep_needs_setup', handleDepNeedsSetup);
    window.addEventListener('nebo:dep_started', handleDepStarted);
    window.addEventListener('nebo:dep_pending', handleDepPending);
    window.addEventListener('nebo:dep_installed', handleDepInstalled);
    window.addEventListener('nebo:dep_failed', handleDepFailed);
    window.addEventListener('nebo:agent_auth_required', handleAgentAuthRequired);
    window.addEventListener('nebo:plugin_auth_url', handlePluginAuthUrl);
    window.addEventListener('nebo:plugin_auth_complete', handlePluginAuthComplete);
    window.addEventListener('nebo:plugin_auth_error', handlePluginAuthError);
  });

  onDestroy(() => {
    window.removeEventListener('nebo:code_processing', handleCodeProcessing);
    window.removeEventListener('nebo:code_result', handleCodeResult);
    window.removeEventListener('nebo:plugin_installing', handlePluginInstalling);
    window.removeEventListener('nebo:plugin_installed', handlePluginInstalled);
    window.removeEventListener('nebo:dep_cascade_start', handleDepCascadeStart);
    window.removeEventListener('nebo:dep_needs_setup', handleDepNeedsSetup);
    window.removeEventListener('nebo:dep_started', handleDepStarted);
    window.removeEventListener('nebo:dep_pending', handleDepPending);
    window.removeEventListener('nebo:dep_installed', handleDepInstalled);
    window.removeEventListener('nebo:dep_failed', handleDepFailed);
    window.removeEventListener('nebo:agent_auth_required', handleAgentAuthRequired);
    window.removeEventListener('nebo:plugin_auth_url', handlePluginAuthUrl);
    window.removeEventListener('nebo:plugin_auth_complete', handlePluginAuthComplete);
    window.removeEventListener('nebo:plugin_auth_error', handlePluginAuthError);
  });
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true" data-modal-open>
    <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="presentation" onclick={close} onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); close(); } }}></div>

    <div class="relative w-full max-w-sm max-h-[85vh] flex flex-col rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden" role="presentation" onkeydown={handleKeydown}>
      <!-- Header -->
      <div class="shrink-0 flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-sm font-semibold">{title}</h3>
        <button
          class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/50 hover:text-base-content transition-colors"
          onclick={close}
          title="Close"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        </button>
      </div>

      <!-- Body (scrolls; header + footer stay pinned) -->
      <div class="px-5 py-6 overflow-y-auto">
        {#if phase === 'installing'}
          <div class="flex flex-col items-center gap-4">
            {#if progressTotal > 1}
              <!-- Multi-dependency install (e.g. a collection): determinate bar. -->
              <div class="w-full">
                <div class="flex items-baseline justify-between mb-2">
                  <p class="text-sm font-medium">{statusMessage}</p>
                  <span class="text-xs text-base-content/50 font-mono">{settledCount}/{progressTotal}</span>
                </div>
                <progress class="progress progress-primary w-full" value={settledCount} max={progressTotal}></progress>
              </div>
            {:else}
              <!-- Single artifact: nothing to count, so an honest spinner. -->
              <span class="loading loading-spinner loading-lg text-primary"></span>
              <div class="text-center">
                <p class="text-sm font-medium">{statusMessage}</p>
                {#if code}
                  <p class="text-xs text-base-content/50 mt-1.5 font-mono">{code}</p>
                {/if}
              </div>
            {/if}
            <button type="button" class="btn btn-sm btn-ghost" onclick={close}>Cancel</button>
          </div>

        {:else if phase === 'auth'}
          <div class="flex flex-col items-center gap-4">
            {#if authQueue.length > 1}
              <p class="text-xs text-base-content/50">Step {authIndex + 1} of {authQueue.length}</p>
            {/if}
            {#if authInProgress}
              <span class="loading loading-spinner loading-lg text-primary"></span>
              <div class="text-center">
                <p class="text-sm font-medium">Waiting for authorization...</p>
                <p class="text-xs text-base-content/50 mt-1.5">Complete the sign-in in your browser, then return here.</p>
              </div>
              <button type="button" class="btn btn-sm btn-ghost mt-2" onclick={() => { authInProgress = false; phase = 'done'; setTimeout(close, 1500); }}>Cancel</button>
            {:else}
              <div class="w-12 h-12 rounded-full bg-primary/15 flex items-center justify-center">
                <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4"/></svg>
              </div>
              <div class="text-center">
                {#if authDescription}
                  <p class="text-xs text-base-content/70 mt-1">{authDescription}</p>
                {/if}
              </div>
              <button type="button" class="btn btn-primary btn-sm mt-2" onclick={startAuth}>
                Connect {authLabel || 'Account'}
              </button>
              <button type="button" class="btn btn-sm btn-ghost" onclick={skipAuth}>Skip</button>
            {/if}
          </div>

        {:else if phase === 'done'}
          <div class="flex flex-col items-center gap-4">
            <div class="w-12 h-12 rounded-full bg-success/15 flex items-center justify-center">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="text-success"><polyline points="20 6 9 17 4 12"/></svg>
            </div>
            <p class="text-sm font-medium">{artifactName || typeLabel} installed!</p>
          </div>

        {:else if phase === 'error'}
          <div class="flex flex-col items-center gap-4">
            <div class="w-12 h-12 rounded-full bg-error/15 flex items-center justify-center">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-error"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>
            </div>
            <div class="text-center">
              <p class="text-sm font-medium">Failed to install</p>
              <p class="text-xs text-error/80 mt-2 max-w-[280px]">{errorMessage}</p>
            </div>
          </div>

        {:else if phase === 'confirm'}
          <div class="flex flex-col items-center gap-4">
            <!-- Artifact info -->
            <div class="w-12 h-12 rounded-full bg-primary/15 flex items-center justify-center">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><rect x="1" y="4" width="22" height="16" rx="2"/><line x1="1" y1="10" x2="23" y2="10"/></svg>
            </div>
            <div class="text-center">
              <p class="text-sm font-medium">{artifactName || typeLabel}</p>
              <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70 mt-1 inline-block">{artifactType || codeType}</span>
            </div>

            <!-- Price breakdown -->
            {#if tier}
              <div class="w-full rounded-xl bg-base-200/50 border border-base-content/10 p-4">
                {#if tier.name}
                  <p class="text-xs text-base-content/50 mb-1">{tier.name}</p>
                {/if}
                <p class="text-xl font-bold text-base-content">
                  {formatPrice(tier.recurringPriceCents || 0, tier.billingInterval)}
                </p>
                {#if tier.pricingModel === 'perBot'}
                  <p class="text-xs text-base-content/50 mt-1">per agent</p>
                {/if}
              </div>
            {/if}

            <!-- Payment method -->
            <div class="w-full rounded-xl bg-base-200/50 border border-base-content/10 p-4">
              {#if paymentMethodLoading}
                <div class="flex items-center gap-2">
                  <span class="loading loading-spinner loading-xs text-base-content/50"></span>
                  <span class="text-xs text-base-content/50">Loading payment info...</span>
                </div>
              {:else if paymentMethod}
                <div class="flex items-center gap-2">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-base-content/60 shrink-0"><rect x="1" y="4" width="22" height="16" rx="2"/><line x1="1" y1="10" x2="23" y2="10"/></svg>
                  <span class="text-sm text-base-content">{paymentMethod.brand || paymentMethod.type} ending in {paymentMethod.lastFour || '****'}</span>
                </div>
              {:else}
                <p class="text-xs text-base-content/50">Payment method will be collected at checkout</p>
              {/if}
            </div>

            <!-- Actions -->
            <div class="flex flex-col gap-2 w-full mt-1">
              <button
                type="button"
                class="btn btn-primary btn-sm w-full"
                onclick={confirmPurchase}
                disabled={confirmLoading || paymentMethodLoading}
              >
                {#if confirmLoading}
                  <span class="loading loading-spinner loading-xs"></span>
                {:else}
                  Confirm Purchase{tier ? ` — ${formatPrice(tier.recurringPriceCents || 0, tier.billingInterval)}` : ''}
                {/if}
              </button>
              <button type="button" class="btn btn-sm btn-ghost w-full" onclick={close}>Cancel</button>
            </div>

            <p class="text-xs text-base-content/40 text-center">You can cancel anytime from Settings → Billing</p>
          </div>

        {:else if phase === 'processing'}
          <div class="flex flex-col items-center gap-4">
            <span class="loading loading-spinner loading-lg text-primary"></span>
            <div class="text-center">
              <p class="text-sm font-medium">{statusMessage || 'Processing payment...'}</p>
              <p class="text-xs text-base-content/50 mt-1.5">This may take a moment</p>
            </div>
            <button type="button" class="btn btn-sm btn-ghost" onclick={close}>Cancel</button>
          </div>

        {:else if phase === 'checkout'}
          <div class="flex flex-col items-center gap-4">
            <div bind:this={stripeContainerEl} class="w-full min-h-[200px]"></div>
            <button type="button" class="btn btn-sm btn-ghost" onclick={close}>Cancel</button>
          </div>
        {/if}

        <!-- Dependency list -->
        {#if deps.length > 0}
          <div class="border-t border-base-content/10 pt-4 mt-5">
            <p class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">
              Dependencies ({installedCount}/{progressTotal})
              {#if failedCount > 0}<span class="text-error/70 normal-case font-medium"> · {failedCount} failed</span>{/if}
            </p>
            <ul class="flex flex-col gap-2">
              {#each deps as dep}
                <li class="flex items-center gap-2.5 text-xs">
                  {#if dep.status === 'installed'}
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="text-success shrink-0"><polyline points="20 6 9 17 4 12"/></svg>
                  {:else if dep.status === 'installing'}
                    <span class="loading loading-spinner loading-xs text-primary shrink-0"></span>
                  {:else if dep.status === 'failed'}
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-error shrink-0"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>
                  {:else}
                    <div class="w-4 h-4 rounded-full border-2 border-base-content/20 shrink-0"></div>
                  {/if}

                  <div class="flex-1 min-w-0">
                    {#if dep.name}
                      <div class="truncate font-medium {dep.status === 'failed' ? 'text-error/90' : ''}">{dep.name}</div>
                      <button
                        type="button"
                        class="font-mono text-base-content/40 hover:text-base-content/70 cursor-pointer bg-transparent border-none p-0"
                        title="Copy install code"
                        onclick={() => copyCode(dep.reference)}
                      >{copiedRef === dep.reference ? 'Copied ✓' : dep.reference}</button>
                    {:else}
                      <button
                        type="button"
                        class="truncate font-mono hover:text-base-content/70 cursor-pointer bg-transparent border-none p-0 {dep.status === 'failed' ? 'text-error/90' : ''}"
                        title="Copy install code"
                        onclick={() => copyCode(dep.reference)}
                      >{copiedRef === dep.reference ? 'Copied ✓' : dep.reference}</button>
                    {/if}
                  </div>

                  <span class="text-xs text-base-content/40 shrink-0">{dep.type}</span>

                  {#if dep.status === 'failed'}
                    <button type="button" class="btn btn-xs btn-primary shrink-0" onclick={() => retryDep(dep)} title={dep.error}>Install</button>
                  {/if}
                </li>
              {/each}
            </ul>
          </div>
        {/if}

        <!-- Needs setup: installed items still missing credentials/config -->
        {#if needsSetup.length > 0}
          <div class="border-t border-base-content/10 pt-4 mt-5">
            <p class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">Needs setup</p>
            <ul class="flex flex-col gap-2">
              {#each needsSetup as item}
                <li class="flex items-center gap-2.5 text-xs">
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-warning shrink-0"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>
                  <div class="flex-1 min-w-0">
                    <div class="truncate font-medium">{item.label || item.slug}</div>
                    {#if item.description}<div class="text-base-content/50 truncate">{item.description}</div>{/if}
                  </div>
                </li>
              {/each}
            </ul>
            <button type="button" class="btn btn-xs btn-outline mt-3" onclick={openPluginSettings}>Configure in Settings</button>
          </div>
        {/if}
      </div>

      <!-- Footer -->
      {#if phase === 'error'}
        <div class="shrink-0 flex justify-end px-5 py-3 border-t border-base-content/10">
          <button type="button" class="btn btn-sm btn-ghost" onclick={close}>Close</button>
        </div>
      {:else if phase === 'done' && interactive}
        <div class="shrink-0 flex justify-end px-5 py-3 border-t border-base-content/10">
          <button type="button" class="btn btn-sm btn-primary" onclick={close}>Done</button>
        </div>
      {/if}
    </div>
  </div>
{/if}
