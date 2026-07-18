<!--
  InstallFlowModal — the ONE install + setup flow for every entry point.

  Replaces the old CodeInstallModal (paste-a-code) and AgentSetupModal (marketplace
  wizard). Three launch modes converge on one phase machine:

    • code      — WS-driven: opens on a pasted install code (nebo:code_processing).
    • product   — API-driven: caller sets appId + show; we call installStoreProduct.
    • configure — edit an already-installed agent (existingAgentId); no install.

  Phase machine (conditional steps shown only when applicable):
    installing → [confirm → processing]            (payment, code mode only)
              → loadSetup(agentId)                  (the join point)
              → [inputs] → [auth] → [schedule] → finalize → done

  The backend force-installs declared deps at install time, so the dependency
  cascade always settles via dep_* events — this modal just renders progress; it
  never force-installs (the only retry path is the per-row Install button →
  approveDeps for a failed dep). "Skip setup" activates immediately with defaults
  and leaves any unfilled inputs / missing plugin auth flagged in Settings.
-->

<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, onDestroy } from 'svelte';
  import { goto } from '$app/navigation';
  import {
    approveDeps,
    authLogin,
    neboAIBillingPaymentMethods,
    installStoreProduct,
    activateAgent,
    getAgent,
    updateAgentInputs,
    listAgentWorkflows,
    updateAgentWorkflow,
  } from '$lib/api/nebo';
  import { createMarketplaceSubscription } from '$lib/api/index';
  import type { PaymentMethodInfo, AgentWorkflow } from '$lib/api/neboComponents';
  import type { AgentInputField } from '$lib/types/agentPage';
  import AgentInputForm from '$lib/components/agent/AgentInputForm.svelte';
  import { installFlow } from '$lib/stores/installFlow';
  import { getWebSocketClient } from '$lib/websocket/client';

  type Phase =
    | 'installing'
    | 'confirm'
    | 'processing'
    | 'inputs'
    | 'schedule'
    | 'done'
    | 'error';
  type Mode = 'code' | 'product' | 'configure';

  type DepItem = {
    reference: string;
    type: string;
    name?: string;
    slug?: string;
    status: 'pending' | 'installing' | 'installed' | 'failed';
    error?: string;
  };
  type TierInfo = {
    name?: string;
    recurringPriceCents?: number;
    billingInterval?: string;
    pricingModel?: string;
  };
  // agentId: the agent that declared this plugin (set by the backend's
  // sweep_plugin_auth). Channel plugins bind per-agent, so we route their setup
  // to this agent rather than defaulting to the primary.
  type AuthEntry = { slug: string; label: string; description: string; authType?: string; agentId?: string };

  // ── Single global instance: no props ────────────────────────────────────────
  // Product/configure opens arrive through the installFlow store; code-paste
  // installs arrive through window `nebo:code_*` events. Either way this one
  // mounted modal owns its own visibility, so two modals can never stack.
  let show = $state(false);
  let mode = $state<Mode>('code');
  let appId = $state('');
  let existingAgentId = $state('');
  let agentName = $state('');
  let agentDescription = $state('');
  let seedInputs = $state<Record<string, unknown> | Record<string, unknown>[]>({});
  let dependencies = $state<unknown>(undefined);
  let oncomplete = $state<((agentId?: string) => void) | undefined>(undefined);
  let onUninstall = $state<(() => void) | undefined>(undefined);

  const configuring = $derived(mode === 'configure');

  let phase = $state<Phase>('installing');
  let code = $state('');
  let codeType = $state('');
  let artifactName = $state('');
  let artifactType = $state('');
  let statusMessage = $state('');
  let errorMessage = $state('');
  let interactive = $state(true);

  // The installed agent we configure + activate. Set from code_result (code),
  // installStoreProduct (product), or existingAgentId (configure).
  let agentId = $state('');

  // Dependency cascade rendering.
  let deps = $state<DepItem[]>([]);
  let depTotal = $state(0);
  let copiedRef = $state('');
  // Cascade lifecycle: the modal only declares success once the backend's terminal
  // `dep_cascade_complete` arrives — never at 0/N while deps are still installing.
  let cascadeStarted = $state(false);
  let cascadeComplete = $state(false);

  // Inputs / auth / schedule setup.
  let inputFields = $state<AgentInputField[]>([]);
  let inputValues = $state<Record<string, unknown>>({});
  let inputsCollected = $state(false);
  let workflows = $state<AgentWorkflow[]>([]);
  let scheduleOverrides = $state<Record<string, string>>({});
  let needsSetupFlag = $state(false);

  // Plugin auth — per-row on the review (done) screen, recomputed after the cascade.
  // authNeeded: slug → entry for installed plugins still needing auth.
  // authState: slug → connection state. connectingSlug: the one OAuth flow in flight
  // (the plugin_auth_* WS events carry no slug, so only one runs at a time).
  let authNeeded = $state<Record<string, AuthEntry>>({});
  let authState = $state<Record<string, 'idle' | 'connecting' | 'connected' | 'failed'>>({});
  let connectingSlug = $state<string | null>(null);

  // Payment (code mode only).
  let tier = $state<TierInfo | null>(null);
  let paymentMethod = $state<PaymentMethodInfo | null>(null);
  let paymentMethodLoading = $state(false);
  let confirmLoading = $state(false);
  let artifactId = $state('');

  // Guards.
  let launched = false;
  let installTimeout: ReturnType<typeof setTimeout> | null = null;
  let copyTimeout: ReturnType<typeof setTimeout> | null = null;

  const KIND_KEYS: Record<string, string> = {
    agent: 'marketplace.kind.agent',
    app: 'marketplace.kind.app',
    skill: 'marketplace.kind.skill',
    plugin: 'marketplace.kind.plugin',
    connector: 'marketplace.kind.connector',
    workflow: 'marketplace.kind.workflow',
    collection: 'marketplace.kind.collection',
  };
  const typeLabel = $derived(
    codeType
      ? KIND_KEYS[codeType]
        ? $t(KIND_KEYS[codeType])
        : codeType.charAt(0).toUpperCase() + codeType.slice(1)
      : $t('marketplace.kind.agent'),
  );
  const installedCount = $derived(deps.filter((d) => d.status === 'installed').length);
  const failedCount = $derived(deps.filter((d) => d.status === 'failed').length);
  const settledCount = $derived(installedCount + failedCount);
  const progressTotal = $derived(Math.max(depTotal, deps.length));
  // True while a cascade has begun but not yet reported complete — gates the
  // "installed!" success state so it never shows at 0/N.
  const cascadePending = $derived(cascadeStarted && !cascadeComplete);
  const hasSchedules = $derived(
    Array.isArray(workflows) &&
      workflows.some((w) => w.isActive && (w.triggerType === 'schedule' || w.triggerType === 'heartbeat')),
  );

  const title = $derived(
    phase === 'done'
      ? configuring ? $t('common.saved') : $t('installFlow.installedTitle', { values: { name: artifactName || typeLabel } })
      : phase === 'error'
        ? $t('installFlow.installFailed')
        : phase === 'confirm'
          ? $t('installFlow.confirmPurchase')
          : phase === 'processing'
            ? $t('installFlow.processingPayment')
            : phase === 'inputs'
              ? configuring ? $t('installFlow.configureTitle', { values: { name: agentName || '' } }) : $t('installFlow.setupTitle', { values: { name: artifactName || agentName || '' } })
              : phase === 'schedule'
                ? $t('installFlow.scheduleTitle')
                : $t('installFlow.installingTitle', { values: { type: typeLabel } }),
  );

  const intervalOptions = $derived([
    { value: '5m', label: $t('automations.every5min') },
    { value: '10m', label: $t('automations.every10min') },
    { value: '15m', label: $t('automations.every15min') },
    { value: '30m', label: $t('automations.every30min') },
    { value: '1h', label: $t('automations.everyHour') },
    { value: '2h', label: $t('automations.every2h') },
    { value: '4h', label: $t('automations.every4h') },
    { value: '8h', label: $t('automations.every8h') },
    { value: '24h', label: $t('automations.every24h') },
  ]);

  function formatPrice(cents: number, interval?: string): string {
    const amount = new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: 'usd',
      minimumFractionDigits: 0,
    }).format(cents / 100);
    if (interval === 'year') return $t('installFlow.pricePerYear', { values: { amount } });
    if (interval === 'month') return $t('installFlow.pricePerMonth', { values: { amount } });
    return amount;
  }

  /** Normalize a marketplace inputs blob (array or object) into AgentInputField[]. */
  function normalizeInputs(raw: Record<string, unknown> | Record<string, unknown>[]): AgentInputField[] {
    if (Array.isArray(raw)) {
      return raw.map((f: any) => ({
        key: f.key || f.name || '',
        label:
          f.label ||
          (f.name || '').replace(/[_-]/g, ' ').replace(/\b\w/g, (c: string) => c.toUpperCase()),
        description: f.description || '',
        type: f.type || 'text',
        required: f.required || false,
        default: f.default,
        placeholder: f.placeholder || '',
        options: Array.isArray(f.options)
          ? f.options.map((o: any) =>
              typeof o === 'string'
                ? { value: o, label: o.replace(/[_-]/g, ' ').replace(/\b\w/g, (c: string) => c.toUpperCase()) }
                : o,
            )
          : f.options,
      }));
    }
    return Object.keys(raw || {}).map((key) => ({
      key,
      label: key.replace(/[_-]/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase()),
      description: '',
      type: 'text',
      required: false,
    }));
  }

  function reset() {
    phase = 'installing';
    code = '';
    codeType = '';
    artifactName = '';
    artifactType = '';
    statusMessage = '';
    errorMessage = '';
    interactive = true;
    agentId = '';
    deps = [];
    depTotal = 0;
    cascadeStarted = false;
    cascadeComplete = false;
    inputFields = [];
    inputValues = {};
    inputsCollected = false;
    workflows = [];
    scheduleOverrides = {};
    needsSetupFlag = false;
    authNeeded = {};
    authState = {};
    connectingSlug = null;
    tier = null;
    paymentMethod = null;
    paymentMethodLoading = false;
    confirmLoading = false;
    artifactId = '';
  }

  function close() {
    if (installTimeout) {
      clearTimeout(installTimeout);
      installTimeout = null;
    }
    show = false;
    launched = false;
    mode = 'code'; // back to default so a later code-paste install is handled
    installFlow.close();
  }

  function autoCloseIfRemote(delay = 1500) {
    if (!interactive) setTimeout(close, delay);
  }

  // ── Launch ──────────────────────────────────────────────────────────────
  // Every open arrives via the installFlow store. Code-paste opens optimistically
  // (mode 'code') and then the backend's code_*/dep_* WS events drive progress;
  // product/configure copy the payload in and launch once. The roster updates
  // itself off the backend's agent_installed/uninstalled WS broadcast — no local
  // refresh dispatch needed (one pathway, CODE_AUDITOR Rule 8).
  $effect(() => {
    const f = $installFlow;
    if (!f.show || launched) return;
    if (f.mode === 'code') {
      launched = true;
      mode = 'code';
      openCodeFlow({
        code: f.code,
        code_type: f.codeType,
        status_message: f.statusMessage,
        interactive: f.interactive,
      });
      return;
    }
    if (f.mode !== 'product' && f.mode !== 'configure') return;
    mode = f.mode;
    appId = f.appId ?? '';
    existingAgentId = f.existingAgentId ?? '';
    agentName = f.agentName ?? '';
    agentDescription = f.agentDescription ?? '';
    seedInputs = f.seedInputs ?? {};
    dependencies = f.dependencies;
    oncomplete = f.oncomplete;
    onUninstall = f.onUninstall;
    launched = true;
    interactive = true;
    codeType = 'agent';
    show = true;
    void startCallerDriven();
  });

  async function startCallerDriven() {
    reset();
    launched = true;
    show = true;
    artifactName = agentName;
    // Prefill inputs immediately from the seed so the form has labels to render.
    inputValues = Array.isArray(seedInputs) ? {} : { ...(seedInputs as Record<string, unknown>) };
    try {
      if (mode === 'configure') {
        agentId = existingAgentId;
      } else {
        statusMessage = $t('marketplace.detail.installing');
        phase = 'installing';
        if (dependencies) seedDepRows(dependencies);
        const res = await installStoreProduct(appId);
        agentId = (res as any)?.agentId || appId;
      }
      await loadSetup(agentId);
    } catch (e: any) {
      errorMessage = e?.error || e?.message || $t('installFlow.failedToInstall');
      phase = 'error';
    }
  }

  /** Seed dep rows from a marketplace `dependencies` object so rows appear instantly. */
  function seedDepRows(dep: unknown) {
    const d = dep as any;
    if (!d || typeof d !== 'object') return;
    for (const [key, type] of [
      ['agents', 'agent'],
      ['skills', 'skill'],
      ['plugins', 'plugin'],
      ['workflows', 'workflow'],
    ] as const) {
      const arr = d[key];
      if (!Array.isArray(arr)) continue;
      for (const item of arr) {
        const reference = typeof item === 'string' ? item : item?.qualifiedName || item?.id || '';
        if (!reference) continue;
        const name = item && typeof item === 'object' ? item.name : undefined;
        // Seed the canonical slug so a later cascade event (keyed by bare slug)
        // updates THIS row instead of creating a duplicate.
        const slug = item && typeof item === 'object' && item.slug ? item.slug : simpleName(reference);
        findOrAddDep(reference, type, name, slug);
      }
    }
  }

  // ── The join point: load setup data, then route through the wizard ────────
  async function loadSetup(id: string) {
    agentId = id;
    try {
      const a = await getAgent(id);
      if (Array.isArray(a?.inputFields) && a.inputFields.length > 0) {
        inputFields = a.inputFields as AgentInputField[];
      } else if (!Array.isArray(seedInputs) && Object.keys(seedInputs).length > 0) {
        inputFields = normalizeInputs(seedInputs);
      } else if (Array.isArray(seedInputs) && seedInputs.length > 0) {
        inputFields = normalizeInputs(seedInputs);
      }
      // Pre-fill the form with the agent's SAVED inputs so configure mode shows
      // what's stored (not blank/placeholders). getAgent returns inputValues at
      // the top level; merge over any seed defaults.
      const savedRaw = (a as { inputValues?: unknown })?.inputValues;
      const saved = typeof savedRaw === 'string' ? JSON.parse(savedRaw || '{}') : (savedRaw ?? {});
      if (saved && typeof saved === 'object') {
        inputValues = { ...inputValues, ...(saved as Record<string, unknown>) };
      }
      needsSetupFlag = !!(a as any)?.needsSetup;
      // Auth is recomputed AFTER the cascade installs the plugins (see
      // refreshAuthNeeded / handleDepCascadeComplete) — checking here is too early,
      // the just-cascaded plugins aren't installed yet, so their auth never showed.
    } catch {
      /* getAgent may not be ready; fall back to seeds */
      if (!Array.isArray(seedInputs) && Object.keys(seedInputs).length > 0) {
        inputFields = normalizeInputs(seedInputs);
      }
    }
    try {
      const wf = await listAgentWorkflows(id);
      workflows = Array.isArray(wf?.workflows) ? (wf.workflows as AgentWorkflow[]) : [];
    } catch {
      workflows = [];
    }
    routeAfterInstall();
  }

  /** After install + loadSetup: show inputs, then auth, then schedule, then finalize. */
  function routeAfterInstall() {
    // Configure mode always lands on the editable step — even with no input
    // fields — so the Uninstall action and a Save are reachable (otherwise
    // "Configure" on a no-config agent would skip straight to a pointless "Saved!").
    if (configuring) {
      phase = 'inputs';
      return;
    }
    if (inputFields.length > 0 && !inputsCollected) {
      phase = 'inputs';
      return;
    }
    continueAfterInputs();
  }

  function continueAfterInputs() {
    // No sequential auth phase any more — plugin connect happens per-row on the
    // review (done) screen, the single auth surface (CODE_AUDITOR Rule 8).
    if (hasSchedules) {
      phase = 'schedule';
      return;
    }
    void finalize();
  }

  async function submitInputs() {
    inputsCollected = true;
    if (agentId && Object.keys(inputValues).length > 0) {
      await updateAgentInputs(agentId, inputValues).catch(() => {});
    }
    continueAfterInputs();
  }

  async function finalize() {
    if (agentId) await activateAgent(agentId).catch(() => {});
    statusMessage = configuring ? $t('common.saved') : $t('installFlow.installedStatus', { values: { name: artifactName || typeLabel } });
    phase = 'done';
    // Surface any plugins still needing auth on the review screen. (Also refreshed
    // on dep_cascade_complete; this covers agents with no cascade.)
    void refreshAuthNeeded();
    if (interactive) {
      // Wait for the user to dismiss (footer Done button).
    } else {
      autoCloseIfRemote();
    }
  }

  /** Skip the remaining setup: activate now with defaults; flag the rest for later. */
  async function skipSetup() {
    if (agentId) await activateAgent(agentId).catch(() => {});
    phase = 'done';
    void refreshAuthNeeded();
  }

  /** Re-fetch which installed plugins still need auth (after the cascade installs
   *  them). The pre-cascade check in loadSetup missed just-installed plugins. */
  async function refreshAuthNeeded() {
    if (!agentId) return;
    try {
      const a = await getAgent(agentId);
      const pna = (a as { pluginsNeedingAuth?: AuthEntry[] })?.pluginsNeedingAuth;
      if (!Array.isArray(pna)) return;
      const needed: Record<string, AuthEntry> = {};
      const next = { ...authState };
      for (const e of pna) {
        if (!e?.slug) continue;
        needed[e.slug] = e;
        if (!next[e.slug]) next[e.slug] = 'idle';
      }
      authNeeded = needed;
      authState = next;
    } catch {
      /* leave existing auth state */
    }
  }

  /** Connect one plugin's account via OAuth. Only one flow runs at a time — the
   *  plugin_auth_* WS events carry no slug, so we map completion to connectingSlug. */
  async function connectPlugin(slug: string) {
    if (connectingSlug) return;
    connectingSlug = slug;
    authState = { ...authState, [slug]: 'connecting' };
    try {
      await authLogin(slug);
    } catch {
      authState = { ...authState, [slug]: 'failed' };
      connectingSlug = null;
    }
  }

  // ── Schedule step ─────────────────────────────────────────────────────────
  function summarizeTrigger(wf: AgentWorkflow): string {
    if (wf.triggerType === 'heartbeat') {
      const interval = wf.triggerConfig.split('|')[0] || '30m';
      return (
        intervalOptions.find((o) => o.value === interval)?.label ||
        $t('automations.everyInterval', { values: { interval } })
      );
    }
    if (wf.triggerType === 'schedule') return $t('installFlow.scheduledAt', { values: { config: wf.triggerConfig } });
    return wf.triggerType;
  }

  async function applySchedules() {
    for (const [bindingName, interval] of Object.entries(scheduleOverrides)) {
      const wf = workflows.find((w) => w.bindingName === bindingName);
      if (!wf || wf.triggerType !== 'heartbeat') continue;
      const parts = wf.triggerConfig.split('|');
      await updateAgentWorkflow(agentId, bindingName, {
        triggerType: 'heartbeat',
        triggerConfig: { interval, ...(parts[1] ? { window: parts[1] } : {}) },
      }).catch(() => {});
    }
    await finalize();
  }

  // ── Plugin auth ────────────────────────────────────────────────────────────
  // (connectPlugin + refreshAuthNeeded live above; per-row on the review screen.)
  // Channel plugins (Slack, etc.) bind per-agent, so their credentials must be
  // entered against the agent that declared them — not the global plugins page,
  // which would default to the primary. `sweep_plugin_auth` tags each item with
  // the declaring `agentId`; route there so a collection's secondary agent owns
  // its channel. Falls back to the global page when no agent is attributed.
  function openPluginSettings(authItem?: { agentId?: string }) {
    close();
    if (authItem?.agentId) {
      goto(`/${authItem.agentId}/settings/configure`);
    } else {
      goto('/settings/plugins');
    }
  }

  // ── Code-mode flow ──────────────────────────────────────────────────────────
  // Shared by the optimistic store open (installFlow.openCode) and the backend's
  // `code_processing` WS frame — both converge here so there's one open path.
  function openCodeFlow(data: {
    code?: string;
    code_type?: string;
    status_message?: string;
    interactive?: boolean;
  }) {
    if (mode !== 'code') return;
    reset();
    code = data?.code || '';
    codeType = data?.code_type || '';
    statusMessage = data?.status_message || $t('installFlow.processing');
    interactive = data?.interactive !== false;
    show = true;

    if (installTimeout) clearTimeout(installTimeout);
    // ponytail: 10s, not 30s — this only fires when code_result is lost; a normal
    // round-trip is <3s. Real cause of a slow spinner is a missing/late code_result.
    installTimeout = setTimeout(() => {
      if (phase === 'installing') {
        statusMessage = $t('installFlow.installedFinalizing', { values: { type: typeLabel } });
        phase = 'done';
        autoCloseIfRemote(2000);
      }
    }, 10_000);
  }

  function handleCodeProcessing(e: Event) {
    openCodeFlow((e as CustomEvent).detail);
  }

  async function handleCodeResult(e: Event) {
    if (mode !== 'code') return;
    if (installTimeout) {
      clearTimeout(installTimeout);
      installTimeout = null;
    }
    const data = (e as CustomEvent).detail;
    const success = data?.success as boolean;
    const paymentRequired = data?.payment_required as boolean;
    const name = (data?.artifact_name as string) || '';
    const id = (data?.artifact_id as string) || '';
    const aType = (data?.artifact_type as string) || '';
    const message = (data?.message as string) || '';
    const tierData = data?.tier as TierInfo | undefined;

    if (name) artifactName = name;
    if (id) artifactId = id;
    if (aType) artifactType = aType;

    if (paymentRequired) {
      if (tierData) tier = tierData;
      void fetchPaymentMethodAndConfirm();
      return;
    }
    if (!success) {
      errorMessage = (data?.error as string) || $t('installFlow.installationFailed');
      phase = 'error';
      return;
    }

    // Only agents/apps have a setup wizard. Everything else just finishes.
    if ((codeType === 'agent' || codeType === 'app') && id) {
      await loadSetup(id);
    } else {
      statusMessage = message || $t('installFlow.installedStatus', { values: { name: artifactName || typeLabel } });
      phase = 'done';
      autoCloseIfRemote();
    }
  }

  async function fetchPaymentMethodAndConfirm() {
    paymentMethodLoading = true;
    phase = 'confirm';
    try {
      const resp = await neboAIBillingPaymentMethods();
      const methods = (resp as any)?.methods as PaymentMethodInfo[] | undefined;
      paymentMethod = methods?.find((m) => m.isDefault) || methods?.[0] || null;
    } catch {
      paymentMethod = null;
    } finally {
      paymentMethodLoading = false;
    }
  }

  async function confirmPurchase() {
    confirmLoading = true;
    try {
      const resp = await createMarketplaceSubscription({
        targetId: artifactId,
        targetType: artifactType || codeType,
        botCount: 1,
      });
      if (resp.checkoutUrl) {
        window.open(resp.checkoutUrl, '_blank');
        phase = 'processing';
        statusMessage = $t('installFlow.completePaymentInBrowser');
      } else {
        phase = 'processing';
        statusMessage = $t('installFlow.finalizing');
      }
    } catch (e: any) {
      errorMessage = e?.message || $t('installFlow.failedToStartCheckout');
      phase = 'error';
    } finally {
      confirmLoading = false;
    }
  }

  // ── Dependency cascade rendering (shared by all modes) ───────────────────────
  /** Last segment of a qualified ref (@org/plugins/gws → gws); else the ref itself. */
  function simpleName(ref: string): string {
    return ref.startsWith('@') && ref.includes('/') ? ref.split('/').pop() || ref : ref;
  }
  /** Canonical identity for a dep — its slug if known, else the simple name. The seed
   *  (qualified ref) and the cascade event (bare slug) for the same plugin must share
   *  this key, or they'd render as two rows (one stuck pending). */
  function depKey(reference: string, slug?: string): string {
    return slug && slug.length ? slug : simpleName(reference);
  }
  function findOrAddDep(reference: string, type: string, name?: string, slug?: string): number {
    const key = depKey(reference, slug);
    const idx = deps.findIndex((d) => depKey(d.reference, d.slug) === key);
    if (idx >= 0) {
      // Backfill richer metadata (display name / slug) from whichever source has it.
      if ((name && !deps[idx].name) || (slug && !deps[idx].slug)) {
        deps[idx] = { ...deps[idx], name: deps[idx].name ?? name, slug: deps[idx].slug ?? slug };
      }
      return idx;
    }
    deps = [...deps, { reference, type, name, slug, status: 'pending' }];
    return deps.length - 1;
  }

  function handleDepCascadeStart(e: Event) {
    if (!show) return;
    cascadeStarted = true;
    const total = Number((e as CustomEvent).detail?.total ?? 0);
    if (total > 0) depTotal = total;
  }
  function handleDepCascadeComplete() {
    if (!show) return;
    cascadeComplete = true;
    // Plugins are installed now — surface any that still need auth on the review row.
    void refreshAuthNeeded();
  }
  function handleDepStarted(e: Event) {
    if (!show) return;
    const d = (e as CustomEvent).detail;
    if (!d?.reference) return;
    const idx = findOrAddDep(d.reference, (d.depType || 'skill').toLowerCase(), d.name, d.slug);
    if (deps[idx].status === 'pending') deps[idx] = { ...deps[idx], status: 'installing' };
  }
  function handleDepPending(e: Event) {
    if (!show) return;
    const d = (e as CustomEvent).detail;
    if (d?.reference) findOrAddDep(d.reference, (d.depType || 'skill').toLowerCase(), d.name, d.slug);
  }
  function handleDepInstalled(e: Event) {
    if (!show) return;
    const d = (e as CustomEvent).detail;
    if (!d?.reference) return;
    const idx = findOrAddDep(d.reference, (d.depType || 'skill').toLowerCase(), d.name, d.slug);
    deps[idx] = { ...deps[idx], status: 'installed', error: undefined };
  }
  function handleDepFailed(e: Event) {
    if (!show) return;
    const d = (e as CustomEvent).detail;
    if (!d?.reference) return;
    const idx = findOrAddDep(d.reference, (d.depType || 'skill').toLowerCase(), d.name, d.slug);
    deps[idx] = { ...deps[idx], status: 'failed', error: (d.error as string) || $t('installFlow.unknownError') };
  }
  function handlePluginInstalling(e: Event) {
    if (!show) return;
    const plugin = (e as CustomEvent).detail?.plugin as string;
    if (!plugin) return;
    const idx = findOrAddDep(plugin, 'plugin');
    deps[idx] = { ...deps[idx], status: 'installing' };
  }
  function handlePluginInstalled(e: Event) {
    if (!show) return;
    const plugin = (e as CustomEvent).detail?.plugin as string;
    if (!plugin) return;
    const idx = findOrAddDep(plugin, 'plugin');
    deps[idx] = { ...deps[idx], status: 'installed' };
  }
  function handleDepNeedsSetup(e: Event) {
    if (!show) return;
    // Collections surface plugins-needing-auth via this event (no single agent to
    // getAgent). Merge into the same per-row auth model so there's ONE auth surface.
    const items = ((e as CustomEvent).detail?.items as AuthEntry[]) || [];
    if (!Array.isArray(items) || items.length === 0) return;
    const needed = { ...authNeeded };
    const next = { ...authState };
    for (const it of items) {
      if (!it?.slug) continue;
      needed[it.slug] = it;
      if (!next[it.slug]) next[it.slug] = 'idle';
    }
    authNeeded = needed;
    authState = next;
  }

  async function retryDep(dep: DepItem) {
    const idx = deps.findIndex((d) => d.reference === dep.reference);
    if (idx < 0) return;
    deps[idx] = { ...deps[idx], status: 'installing', error: undefined };
    try {
      await approveDeps({
        deps: [{ depType: dep.type, reference: dep.reference, name: dep.name, slug: dep.slug }],
      });
    } catch {
      deps[idx] = { ...deps[idx], status: 'failed', error: $t('installFlow.retryFailedToStart') };
    }
  }

  // ── Plugin auth WS handlers ─────────────────────────────────────────────────
  // The auth URL is opened ONCE, globally, in listeners.ts. The modal only tracks
  // per-row connect state (connectPlugin) and completion/failure below.
  function handlePluginAuthComplete() {
    if (!show || !connectingSlug) return;
    authState = { ...authState, [connectingSlug]: 'connected' };
    connectingSlug = null;
  }
  function handlePluginAuthError() {
    if (!show || !connectingSlug) return;
    // A failed connect marks just that row failed (retryable) — it never blocks
    // the install or errors the whole modal.
    authState = { ...authState, [connectingSlug]: 'failed' };
    connectingSlug = null;
  }

  // ── Misc UI ─────────────────────────────────────────────────────────────────
  function depLabel(dep: DepItem): string {
    if (dep.name) return dep.name;
    const ref = dep.reference;
    if (ref.startsWith('@') && ref.includes('/')) return ref.split('/').pop() || ref;
    return ref;
  }
  async function copyCode(reference: string) {
    try {
      await navigator.clipboard.writeText(reference);
      copiedRef = reference;
      if (copyTimeout) clearTimeout(copyTimeout);
      copyTimeout = setTimeout(() => (copiedRef = ''), 1500);
    } catch {
      /* clipboard unavailable */
    }
  }
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') close();
  }

  // Subscribe to install/cascade/auth WS events on the single ws.on pathway.
  // These handlers read `(e as CustomEvent).detail`, so adapt the raw payload to
  // that shape. (Set up in onMount, so cleanup is via collected unsubscribes.)
  const wsUnsubs: (() => void)[] = [];
  function subModal(event: string, handler: (e: Event) => void) {
    wsUnsubs.push(
      getWebSocketClient().on(event, (data: unknown) => handler({ detail: data } as unknown as CustomEvent)),
    );
  }

  onMount(() => {
    subModal('code_processing', handleCodeProcessing);
    subModal('code_result', handleCodeResult);
    subModal('plugin_installing', handlePluginInstalling);
    subModal('plugin_installed', handlePluginInstalled);
    subModal('dep_cascade_start', handleDepCascadeStart);
    subModal('dep_cascade_complete', handleDepCascadeComplete);
    subModal('dep_needs_setup', handleDepNeedsSetup);
    subModal('dep_started', handleDepStarted);
    subModal('dep_pending', handleDepPending);
    subModal('dep_installed', handleDepInstalled);
    subModal('dep_failed', handleDepFailed);
    subModal('plugin_auth_complete', handlePluginAuthComplete);
    subModal('plugin_auth_error', handlePluginAuthError);
  });
  onDestroy(() => {
    for (const off of wsUnsubs) off();
    if (installTimeout) clearTimeout(installTimeout);
    if (copyTimeout) clearTimeout(copyTimeout);
  });

  const showSkip = $derived(!configuring && (phase === 'inputs' || phase === 'schedule'));
</script>

{#if show}
  <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true" data-modal-open>
    <div
      class="absolute inset-0 bg-black/60 backdrop-blur-sm"
      role="presentation"
      onclick={close}
      onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); close(); } }}
    ></div>

    <div
      class="relative w-full max-w-sm max-h-[85vh] flex flex-col rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden"
      role="presentation"
      onkeydown={handleKeydown}
    >
      <!-- Header -->
      <div class="shrink-0 flex items-center justify-between px-5 py-4 border-b border-base-content/10">
        <h3 class="text-sm font-semibold">{title}</h3>
        <button
          class="w-7 h-7 rounded-md flex items-center justify-center hover:bg-base-200 cursor-pointer bg-transparent border-none text-base-content/50 hover:text-base-content transition-colors"
          onclick={close}
          title={$t('common.close')}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        </button>
      </div>

      <!-- Body -->
      <div class="px-5 py-6 overflow-y-auto">
        {#if phase === 'installing'}
          <div class="flex flex-col items-center gap-4">
            {#if progressTotal > 1}
              <div class="w-full">
                <div class="flex items-baseline justify-between mb-2">
                  <p class="text-sm font-medium">{statusMessage}</p>
                  <span class="text-xs text-base-content/50 font-mono">{settledCount}/{progressTotal}</span>
                </div>
                <progress class="progress progress-primary w-full" value={settledCount} max={progressTotal}></progress>
              </div>
            {:else}
              <span class="loading loading-spinner loading-lg text-primary"></span>
              <div class="text-center">
                <p class="text-sm font-medium">{statusMessage}</p>
                {#if code}<p class="text-xs text-base-content/50 mt-1.5 font-mono">{code}</p>{/if}
              </div>
            {/if}
          </div>

        {:else if phase === 'inputs'}
          <div class="flex flex-col gap-4">
            {#if agentDescription}<p class="text-sm text-base-content/70">{agentDescription}</p>{/if}
            {#if inputFields.length > 0}
              <AgentInputForm fields={inputFields} bind:values={inputValues} onchange={(v) => (inputValues = v)} />
            {:else}
              <p class="text-sm text-base-content/70 text-center">{$t('installFlow.noConfigNeeded')}</p>
            {/if}
            <button type="button" class="btn btn-primary btn-sm w-full" onclick={submitInputs}>
              {configuring ? $t('installFlow.saveChanges') : $t('common.continue')}
            </button>
            {#if configuring && onUninstall}
              <button type="button" class="btn btn-ghost btn-sm text-error/80 hover:text-error" onclick={onUninstall}>
                {$t('installFlow.uninstallName', { values: { name: agentName } })}
              </button>
            {/if}
          </div>

        {:else if phase === 'schedule'}
          <div class="flex flex-col gap-4">
            <p class="text-sm text-base-content/70 text-center">{$t('installFlow.changeAnytime')}</p>
            {#each workflows.filter((w) => w.isActive && (w.triggerType === 'schedule' || w.triggerType === 'heartbeat')) as wf}
              <div class="rounded-xl border border-base-content/10 p-4">
                <p class="text-sm font-medium mb-1">{wf.description || wf.bindingName}</p>
                <p class="text-xs text-base-content/70 mb-3">{$t('installFlow.currently', { values: { value: summarizeTrigger(wf) } })}</p>
                {#if wf.triggerType === 'heartbeat'}
                  <select
                    class="select select-bordered select-sm w-full"
                    value={scheduleOverrides[wf.bindingName] || wf.triggerConfig.split('|')[0] || '30m'}
                    onchange={(e) => (scheduleOverrides[wf.bindingName] = (e.target as HTMLSelectElement).value)}
                  >
                    {#each intervalOptions as opt}<option value={opt.value}>{opt.label}</option>{/each}
                  </select>
                {:else}
                  <p class="text-xs text-base-content/70">{$t('installFlow.fixedSchedule')}</p>
                {/if}
              </div>
            {/each}
            <button type="button" class="btn btn-primary btn-sm w-full" onclick={applySchedules}>{$t('installFlow.startWorking')}</button>
          </div>

        {:else if phase === 'done'}
          <div class="flex flex-col items-center gap-4">
            {#if cascadePending}
              <span class="loading loading-spinner loading-lg text-primary"></span>
              <p class="text-sm font-medium">
                {$t('installFlow.installingDependencies')} <span class="font-mono text-base-content/50">{settledCount}/{progressTotal}</span>
              </p>
            {:else}
              <div class="w-12 h-12 rounded-full bg-success/15 flex items-center justify-center">
                <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" class="text-success"><polyline points="20 6 9 17 4 12"/></svg>
              </div>
              <p class="text-sm font-medium">{configuring ? $t('installFlow.savedExclaim') : $t('installFlow.installedExclaim', { values: { name: artifactName || typeLabel } })}</p>
              {#if needsSetupFlag && agentId}
                <button type="button" class="btn btn-xs btn-outline" onclick={() => { const id = agentId; close(); goto(`/${id}/settings/configure`); }}>
                  {$t('installFlow.finishSetupInSettings')}
                </button>
              {/if}
            {/if}
          </div>

        {:else if phase === 'error'}
          <div class="flex flex-col items-center gap-4">
            <div class="w-12 h-12 rounded-full bg-error/15 flex items-center justify-center">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-error"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>
            </div>
            <div class="text-center">
              <p class="text-sm font-medium">{$t('installFlow.failedToInstall')}</p>
              <p class="text-xs text-error/80 mt-2 max-w-[280px]">{errorMessage}</p>
            </div>
          </div>

        {:else if phase === 'confirm'}
          <div class="flex flex-col items-center gap-4">
            <div class="w-12 h-12 rounded-full bg-primary/15 flex items-center justify-center">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-primary"><rect x="1" y="4" width="22" height="16" rx="2"/><line x1="1" y1="10" x2="23" y2="10"/></svg>
            </div>
            <div class="text-center">
              <p class="text-sm font-medium">{artifactName || typeLabel}</p>
              <span class="px-1.5 py-0.5 rounded text-xs font-mono bg-base-200 text-base-content/70 mt-1 inline-block">{artifactType || codeType}</span>
            </div>
            {#if tier}
              <div class="w-full rounded-xl bg-base-200/50 border border-base-content/10 p-4">
                {#if tier.name}<p class="text-xs text-base-content/50 mb-1">{tier.name}</p>{/if}
                <p class="text-xl font-bold text-base-content">{formatPrice(tier.recurringPriceCents || 0, tier.billingInterval)}</p>
                {#if tier.pricingModel === 'perBot'}<p class="text-xs text-base-content/50 mt-1">{$t('installFlow.perEmployee')}</p>{/if}
              </div>
            {/if}
            <div class="w-full rounded-xl bg-base-200/50 border border-base-content/10 p-4">
              {#if paymentMethodLoading}
                <div class="flex items-center gap-2"><span class="loading loading-spinner loading-xs text-base-content/50"></span><span class="text-xs text-base-content/50">{$t('installFlow.loadingPaymentInfo')}</span></div>
              {:else if paymentMethod}
                <div class="flex items-center gap-2"><span class="text-sm text-base-content">{$t('settingsBilling.cardEnding', { values: { brand: paymentMethod.brand || paymentMethod.type, lastFour: paymentMethod.lastFour || '****' } })}</span></div>
              {:else}
                <p class="text-xs text-base-content/50">{$t('installFlow.paymentAtCheckout')}</p>
              {/if}
            </div>
            <div class="flex flex-col gap-2 w-full mt-1">
              <button type="button" class="btn btn-primary btn-sm w-full" onclick={confirmPurchase} disabled={confirmLoading || paymentMethodLoading}>
                {#if confirmLoading}<span class="loading loading-spinner loading-xs"></span>{:else}{$t('installFlow.confirmPurchase')}{tier ? ` — ${formatPrice(tier.recurringPriceCents || 0, tier.billingInterval)}` : ''}{/if}
              </button>
              <button type="button" class="btn btn-sm btn-ghost w-full" onclick={close}>{$t('common.cancel')}</button>
            </div>
          </div>

        {:else if phase === 'processing'}
          <div class="flex flex-col items-center gap-4">
            <span class="loading loading-spinner loading-lg text-primary"></span>
            <div class="text-center">
              <p class="text-sm font-medium">{statusMessage || $t('installFlow.processingPaymentEllipsis')}</p>
              <p class="text-xs text-base-content/50 mt-1.5">{$t('installFlow.mayTakeMoment')}</p>
            </div>
            <button type="button" class="btn btn-sm btn-ghost" onclick={close}>{$t('common.cancel')}</button>
          </div>
        {/if}

        <!-- Dependency list (shared; visible whenever there are deps) -->
        {#if deps.length > 0}
          <div class="border-t border-base-content/10 pt-4 mt-5">
            <p class="text-xs font-semibold uppercase tracking-wider text-base-content/50 mb-3">
              {$t('installFlow.dependencies')} ({installedCount}/{progressTotal})
              {#if failedCount > 0}<span class="text-error/70 normal-case font-medium"> · {$t('agentActivity.failedCount', { values: { count: failedCount } })}</span>{/if}
            </p>
            <ul class="flex flex-col gap-2">
              {#each deps as dep}
                {@const label = depLabel(dep)}
                {@const slug = dep.slug ?? simpleName(dep.reference)}
                {@const auth = dep.type === 'plugin' ? authNeeded[slug] : undefined}
                {@const aState = authState[slug] ?? 'idle'}
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
                    <div class="truncate font-medium {dep.status === 'failed' ? 'text-error/90' : ''}">{label}</div>
                    {#if dep.reference !== label}
                      <button type="button" class="font-mono text-base-content/40 hover:text-base-content/70 cursor-pointer bg-transparent border-none p-0" title={$t('installFlow.copyInstallCode')} onclick={() => copyCode(dep.reference)}>
                        {copiedRef === dep.reference ? $t('installFlow.copied') : dep.reference}
                      </button>
                    {/if}
                  </div>
                  {#if auth && aState === 'connected'}
                    <span class="text-xs text-success shrink-0">{$t('common.connected')}</span>
                  {:else}
                    <span class="text-xs text-base-content/40 shrink-0">{dep.type}</span>
                  {/if}
                  {#if dep.status === 'failed'}
                    <button type="button" class="btn btn-xs btn-primary shrink-0" onclick={() => retryDep(dep)} title={dep.error}>{$t('common.install')}</button>
                  {:else if auth && aState !== 'connected'}
                    {#if aState === 'connecting'}
                      <span class="loading loading-spinner loading-xs text-primary shrink-0"></span>
                    {:else if auth.authType === 'env'}
                      <button type="button" class="btn btn-xs btn-outline shrink-0" onclick={() => openPluginSettings(auth)}>{$t('installFlow.setUp')}</button>
                    {:else}
                      <button type="button" class="btn btn-xs btn-primary shrink-0" disabled={!!connectingSlug} onclick={() => connectPlugin(slug)}>{aState === 'failed' ? $t('common.retry') : $t('settingsPlugins.connect')}</button>
                    {/if}
                  {/if}
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>

      <!-- Footer -->
      {#if phase === 'error'}
        <div class="shrink-0 flex justify-end px-5 py-3 border-t border-base-content/10">
          <button type="button" class="btn btn-sm btn-ghost" onclick={close}>{$t('common.close')}</button>
        </div>
      {:else if phase === 'done' && interactive}
        <div class="shrink-0 flex justify-end px-5 py-3 border-t border-base-content/10">
          <button type="button" class="btn btn-sm btn-primary" disabled={cascadePending} onclick={() => { const id = agentId; close(); oncomplete?.(id || undefined); }}>{$t('common.done')}</button>
        </div>
      {:else if showSkip}
        <div class="shrink-0 flex justify-between px-5 py-3 border-t border-base-content/10">
          <button type="button" class="btn btn-sm btn-ghost" onclick={skipSetup}>{$t('installFlow.skipSetup')}</button>
        </div>
      {/if}
    </div>
  </div>
{/if}
