<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { completeOnboarding } from '$lib/stores/onboarding';
  import { logger } from '$lib/monitoring';
  import * as api from '$lib/api/nebo';
  import { neboAIOAuthStartWithJanus, neboAIOAuthStatus } from '$lib/api/index';
  import Check from 'lucide-svelte/icons/check';
  import ArrowRight from 'lucide-svelte/icons/arrow-right';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import Shield from 'lucide-svelte/icons/shield';
  import Globe from 'lucide-svelte/icons/globe';
  import Link from 'lucide-svelte/icons/link';
  import Eye from 'lucide-svelte/icons/eye';
  import AlertTriangle from 'lucide-svelte/icons/alert-triangle';
  import X from 'lucide-svelte/icons/x';
  import ApprovalModal from '$lib/components/ApprovalModal.svelte';

  let step = $state(0);
  let tcAccepted = $state(false);
  let fullAccess = $state(false);
  let showApprovalPreview = $state(false);
  let showEnableModal = $state(false);
  let termsAccepted = $state(false);
  let confirmText = $state('');
  const canConfirm = $derived(termsAccepted && confirmText === 'ENABLE');
  let selectedLocale = $state('en');

  /** Full Access: modal only when enabling. Turning off is immediate.
   *  Implemented as a button (not checkbox) so DaisyUI/browser checkbox
   *  change events can't double-fire and reopen the enable modal. */
  function handleFullAccessToggle() {
    if (fullAccess) {
      fullAccess = false;
      return;
    }
    showEnableModal = true;
  }

  function cancelFullAccessEnable() {
    showEnableModal = false;
    termsAccepted = false;
    confirmText = '';
  }

  function confirmFullAccessEnable() {
    if (!canConfirm) return;
    fullAccess = true;
    showEnableModal = false;
    termsAccepted = false;
    confirmText = '';
  }

  // Language picker hidden for v0.10.0. Set SHOW_LANGUAGE_STEP=true to re-enable.
  const SHOW_LANGUAGE_STEP = false;

  // NeboAI OAuth state
  let neboAILoading = $state(false);
  let neboAIConnected = $state(false);
  let neboAIEmail = $state('');
  let neboAIError = $state('');
  let neboAIPendingState = $state('');
  let neboAIPollInterval: ReturnType<typeof setInterval> | null = null;
  let neboAITimeout: ReturnType<typeof setTimeout> | null = null;

  // Capability labels come from the backend canonical list
  // (userGetPermissions().capabilities) — see tools::capabilities. Not hardcoded
  // here, so onboarding can't drift from Settings or the gate.

  // Safe-by-default for a brand-new, non-technical user: core low-risk capabilities ON,
  // powerful / privacy-sensitive ones OFF (opt-in). The "Acting With Care" layer still
  // confirms individual sensitive actions, so this is the coarse gate, not a usability tax.
  const DEFAULT_ENABLED: Record<string, boolean> = {
    chat: true,
    file: true,
    web: true,
    shell: false,
    desktop: false,
    media: false,
    contacts: false,
    system: false,
  };

  let permissions = $state<{ key: string; label: string; desc: string; enabled: boolean; locked: boolean }[]>([]);
  let capStates = $state<boolean[]>([]);

  onMount(async () => {
    try {
      const res = await api.userGetPermissions();
      let permObj: Record<string, boolean> = {};
      if (res?.permissions?.length) {
        for (const tp of res.permissions) {
          permObj[tp.tool] = tp.allowed;
        }
      }
      permissions = (res?.capabilities ?? []).map(cap => ({
        key: cap.key,
        label: cap.label,
        desc: cap.desc,
        enabled: permObj[cap.key] ?? DEFAULT_ENABLED[cap.key] ?? false,
        locked: false,
      }));
      capStates = permissions.map(p => p.enabled);
    } catch {}
  });


  // Accept Terms & Conditions via backend
  async function acceptTerms() {
    try {
      await api.userAcceptTerms();
    } catch {
      // Non-blocking — user can still proceed
      logger.warn('Failed to record T&C acceptance on backend');
    }
    if (SHOW_LANGUAGE_STEP) {
      step = 1;
    } else {
      // Skip language step; persist default locale
      localStorage.setItem('nebo_locale', 'en');
      try { await api.userUpdatePreferences({ language: 'en' }); } catch {}
      step = 2;
    }
  }

  // Save language preference via backend
  async function saveLocale() {
    localStorage.setItem('nebo_locale', selectedLocale);
    try {
      await api.userUpdatePreferences({ language: selectedLocale });
    } catch {
      logger.warn('Failed to save language preference to backend');
    }
    step = 2;
  }

  // NeboAI OAuth flow
  function cleanupNeboAIOAuth() {
    if (neboAIPollInterval) {
      clearInterval(neboAIPollInterval);
      neboAIPollInterval = null;
    }
    if (neboAITimeout) {
      clearTimeout(neboAITimeout);
      neboAITimeout = null;
    }
  }

  async function connectNeboAI() {
    neboAILoading = true;
    neboAIError = '';

    try {
      // Start OAuth — backend opens browser with OAuth URL
      const result = await neboAIOAuthStartWithJanus(true);
      neboAIPendingState = result.state;

      // Set 3-minute timeout
      neboAITimeout = setTimeout(() => {
        cleanupNeboAIOAuth();
        neboAILoading = false;
        neboAIError = $t('onboardingPage.connectionTimedOut');
      }, 180_000);

      // Poll every 2 seconds for completion
      neboAIPollInterval = setInterval(async () => {
        try {
          const status = await neboAIOAuthStatus(neboAIPendingState);
          if (status?.status === 'complete') {
            cleanupNeboAIOAuth();
            neboAIConnected = true;
            neboAIEmail = status.email ?? '';
            neboAILoading = false;
            logger.info('NeboAI OAuth completed');
          } else if (status?.status === 'error') {
            cleanupNeboAIOAuth();
            neboAILoading = false;
            neboAIError = status.error || $t('onboardingPage.oauthFailed');
          } else if (status?.status === 'expired') {
            cleanupNeboAIOAuth();
            neboAILoading = false;
            neboAIError = $t('onboardingPage.oauthExpired');
          }
          // 'pending' — keep polling
        } catch {
          // Network error during poll — keep trying silently
        }
      }, 2000);
    } catch (err) {
      neboAILoading = false;
      neboAIError = err instanceof Error ? err.message : $t('onboardingPage.oauthStartFailed');
      logger.error('NeboAI OAuth start failed', err);
    }
  }

  // Save permissions via backend
  async function savePermissions() {
    const permObj: Record<string, boolean> = {};
    permissions.forEach((p, i) => {
      permObj[p.key] = capStates[i];
    });

    try {
      await api.userUpdatePermissions({ permissions: permObj });
      await api.updateSettings({ fullAccess });
    } catch {
      logger.warn('Failed to save permissions to backend');
    }
    step = 4;
  }

  async function finish() {
    await completeOnboarding();
    goto('/');
  }

  const steps = SHOW_LANGUAGE_STEP
    ? ['Welcome', 'Language', 'Connect', 'Permissions', 'Done']
    : ['Welcome', 'Connect', 'Permissions', 'Done'];

  const languages = [
    { code: 'en', label: 'English' },
    { code: 'de', label: 'Deutsch' },
    { code: 'es', label: 'Español' },
    { code: 'fr', label: 'Français' },
    { code: 'it', label: 'Italiano' },
    { code: 'pt', label: 'Português' },
    { code: 'pt-BR', label: 'Português (Brasil)' },
    { code: 'nl', label: 'Nederlands' },
    { code: 'sv', label: 'Svenska' },
    { code: 'pl', label: 'Polski' },
    { code: 'tr', label: 'Türkçe' },
    { code: 'ru', label: 'Русский' },
    { code: 'uk', label: 'Українська' },
    { code: 'ar', label: 'العربية' },
    { code: 'he', label: 'עברית' },
    { code: 'hi', label: 'हिन्दी' },
    { code: 'bn', label: 'বাংলা' },
    { code: 'th', label: 'ไทย' },
    { code: 'vi', label: 'Tiếng Việt' },
    { code: 'id', label: 'Bahasa Indonesia' },
    { code: 'ms', label: 'Bahasa Melayu' },
    { code: 'ja', label: '日本語' },
    { code: 'ko', label: '한국어' },
    { code: 'zh-CN', label: '简体中文' },
    { code: 'zh-TW', label: '繁體中文' },
  ];
</script>

<svelte:head><title>{$t('onboarding.welcome.title')}</title></svelte:head>

<!-- Step indicator -->
<div class="flex items-center justify-center gap-2 mb-10">
  {#each steps as s, i}
    <div class="flex items-center gap-2">
      <div class="w-7 h-7 rounded-full flex items-center justify-center text-xs font-bold transition-colors {i < step ? 'bg-success text-success-content' : i === step ? 'bg-primary text-primary-content' : 'bg-base-200 text-base-content/40'}">
        {#if i < step}
          <Check class="w-4 h-4" />
        {:else}
          {i + 1}
        {/if}
      </div>
      {#if i < steps.length - 1}
        <div class="w-6 h-0.5 rounded {i < step ? 'bg-success' : 'bg-base-200'}"></div>
      {/if}
    </div>
  {/each}
</div>

{#if step === 0}
  <!-- Welcome + T&C -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-primary text-primary-content flex items-center justify-center font-mono text-xl font-bold mx-auto mb-5">N</div>
    <h2 class="text-2xl font-bold mb-2">{$t('onboarding.welcome.title')}</h2>
    <p class="text-xs text-base-content/50 mb-6 max-w-sm mx-auto">{$t('onboardingPage.welcomeDescription')}</p>

    <div class="max-w-sm mx-auto rounded-xl border border-base-300 bg-base-200/30 p-4 mb-8 text-left">
      <div class="flex items-start gap-3">
        <Shield class="w-5 h-5 text-warning shrink-0 mt-0.5" />
        <div class="flex-1 min-w-0">
          <div class="text-sm font-medium mb-1">{$t('onboardingPage.termsTitle')}</div>
          <p class="text-xs text-base-content/70 leading-relaxed mb-3">
            {$t('onboardingPage.termsBody')} <a href="/terms" class="text-primary underline">{$t('onboardingPage.termsOfService')}</a> {$t('onboardingPage.termsAnd')} <a href="/privacy" class="text-primary underline">{$t('onboardingPage.privacyPolicy')}</a>.
          </p>
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" class="checkbox checkbox-sm checkbox-primary" bind:checked={tcAccepted} />
            <span class="text-sm font-medium">{$t('onboardingPage.acceptTerms')}</span>
          </label>
        </div>
      </div>
    </div>

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={acceptTerms}
        disabled={!tcAccepted}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none disabled:opacity-40 disabled:cursor-not-allowed"
      >
        {$t('onboarding.welcome.getStarted')} <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>

{:else if step === 1 && SHOW_LANGUAGE_STEP}
  <!-- Language -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-primary/10 flex items-center justify-center mx-auto mb-5">
      <Globe class="w-7 h-7 text-primary" />
    </div>
    <h2 class="text-2xl font-bold mb-2">{$t('onboardingPage.languageTitle')}</h2>
    <p class="text-xs text-base-content/50 mb-6">{$t('onboardingPage.languageDesc')}</p>

    <div class="grid grid-cols-5 gap-1.5 mb-8 max-w-xl mx-auto">
      {#each languages as lang}
        <button
          class="py-2 px-1 rounded-lg text-xs font-medium cursor-pointer border transition-colors truncate {selectedLocale === lang.code
            ? 'bg-primary text-primary-content border-primary'
            : 'bg-base-100 border-base-300 hover:border-base-content/30 hover:bg-base-200/50'}"
          onclick={() => selectedLocale = lang.code}
        >{lang.label}</button>
      {/each}
    </div>

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={() => (step = 0)}
        class="inline-flex items-center gap-2 px-3 py-3 rounded-xl text-sm font-medium text-base-content/60 cursor-pointer hover:text-base-content transition-colors border-none bg-transparent"
      >
        <ArrowLeft class="w-4 h-4" /> {$t('common.back')}
      </button>
      <button
        onclick={saveLocale}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
      >
        {$t('common.continue')} <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>

{:else if step === 2}
  <!-- Connect NeboAI -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-primary/10 flex items-center justify-center mx-auto mb-5">
      <Link class="w-7 h-7 text-primary" />
    </div>
    <h2 class="text-2xl font-bold mb-2">{$t('onboardingPage.connectTitle')}</h2>
    <p class="text-xs text-base-content/50 mb-6">{$t('onboardingPage.connectDesc')}</p>

    <div class="mb-8">
      {#if neboAIConnected}
        <div class="p-4 rounded-xl border border-success/30 bg-success/5 max-w-sm mx-auto">
          <div class="flex items-center justify-center gap-2">
            <Check class="w-5 h-5 text-success" />
            <span class="text-sm font-semibold text-success">{neboAIEmail ? $t('onboardingPage.connectedAs', { values: { email: neboAIEmail } }) : $t('common.connected')}</span>
          </div>
        </div>
      {:else if neboAILoading}
        <div class="max-w-sm mx-auto">
          <button
            disabled
            class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold border-none opacity-80 mb-3"
          >
            <span class="loading loading-spinner loading-sm"></span> {$t('onboardingPage.waitingForAuthorization')}
          </button>
          <p class="text-xs text-base-content/50">{$t('onboardingPage.browserOpened')}</p>
        </div>
      {:else}
        <div class="max-w-sm mx-auto">
          <button
            onclick={connectNeboAI}
            class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
          >
            {$t('onboardingPage.connectWithNeboAI')}
          </button>
          {#if neboAIError}
            <p class="text-xs text-error mt-3">{neboAIError}</p>
          {/if}
        </div>
      {/if}
    </div>

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={() => { cleanupNeboAIOAuth(); step = SHOW_LANGUAGE_STEP ? 1 : 0; }}
        class="inline-flex items-center gap-2 px-3 py-3 rounded-xl text-sm font-medium text-base-content/60 cursor-pointer hover:text-base-content transition-colors border-none bg-transparent"
      >
        <ArrowLeft class="w-4 h-4" /> {$t('common.back')}
      </button>
      {#if neboAIConnected}
        <button
          onclick={() => (step = 3)}
          class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
        >
          {$t('common.continue')} <ArrowRight class="w-4 h-4" />
        </button>
      {:else}
        <button
          onclick={() => { cleanupNeboAIOAuth(); step = 3; }}
          class="inline-flex items-center gap-2 px-3 py-3 rounded-xl text-sm font-medium text-base-content/60 cursor-pointer hover:text-base-content transition-colors border-none bg-transparent"
        >
          {$t('onboardingPage.skipForNow')} <ArrowRight class="w-4 h-4" />
        </button>
      {/if}
    </div>
  </div>

{:else if step === 3}
  <!-- Permissions / Capabilities -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-primary/10 flex items-center justify-center mx-auto mb-5">
      <Shield class="w-7 h-7 text-primary" />
    </div>
    <h2 class="text-2xl font-bold mb-2">{$t('commandPalette.permissions')}</h2>
    <p class="text-xs text-base-content/50 mb-6">{$t('onboardingPage.permissionsDesc')}</p>

    <!-- Full Access -->
    <div class="flex items-center justify-between p-4 rounded-xl border border-base-300 mb-5 max-w-md mx-auto text-left">
      <div>
        <div class="text-sm font-semibold flex items-center gap-2">
          {#if fullAccess}<AlertTriangle class="w-4 h-4 text-warning" />{/if}
          {$t('onboardingPage.fullAccess')}
        </div>
        <div class="text-xs text-base-content/70">{$t('onboardingPage.fullAccessDesc')}</div>
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={fullAccess}
        aria-label={$t('onboardingPage.fullAccess')}
        class="relative inline-flex h-5 w-8 shrink-0 cursor-pointer items-center rounded-full transition-colors {fullAccess ? 'bg-primary' : 'bg-base-300'}"
        onclick={handleFullAccessToggle}
      >
        <span
          class="pointer-events-none inline-block size-3.5 rounded-full bg-base-100 shadow transition-transform {fullAccess ? 'translate-x-[14px]' : 'translate-x-0.5'}"
        ></span>
      </button>
    </div>

    {#if !fullAccess}
      <div class="divide-y divide-base-content/10 mb-5 max-w-md mx-auto text-left max-h-[40vh] overflow-y-auto pr-1">
        {#each permissions as perm, i}
          <div class="flex items-center justify-between py-3">
            <div>
              <div class="text-sm font-medium">{perm.label}</div>
              <div class="text-xs text-base-content/70">{perm.desc}</div>
            </div>
            {#if perm.locked}
              <span class="text-xs text-base-content/50 font-mono shrink-0 ml-4">{$t('onboarding.capabilities.alwaysOn')}</span>
            {:else}
              <input type="checkbox" class="toggle toggle-sm toggle-primary shrink-0 ml-4" bind:checked={capStates[i]} />
            {/if}
          </div>
        {/each}
      </div>
    {:else}
      <div class="rounded-xl bg-warning/10 border border-warning/20 px-4 py-3 mb-5 max-w-md mx-auto text-left">
        <p class="text-xs text-warning font-medium">{$t('onboardingPage.fullAccessActive')}</p>
        <p class="text-xs text-base-content/70 mt-0.5">{$t('settingsPermissions.autonomousActiveDesc')}</p>
      </div>
    {/if}

    <!-- Approval dialog preview -->
    <button
      onclick={() => showApprovalPreview = true}
      class="inline-flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-base-content/60 cursor-pointer hover:text-base-content hover:bg-base-200/50 transition-colors border-none bg-transparent mb-8"
    >
      <Eye class="w-3.5 h-3.5" /> {$t('onboardingPage.previewApproval')}
    </button>

    <ApprovalModal
      bind:show={showApprovalPreview}
      agent={$t('onboardingPage.previewAgentName')}
      actionType="shell_command"
      actionDetail="curl -s https://api.example.com/data | jq '.results[]'"
      actionKey="preview"
    />

    <!-- Full Access Activation Modal -->
    {#if showEnableModal}
      <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
        <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="button" tabindex="-1" aria-label={$t('common.close')} onclick={cancelFullAccessEnable} onkeydown={(e) => { if (e.key === 'Escape') cancelFullAccessEnable(); }}></div>
        <div class="relative w-full max-w-lg rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden text-left">
          <!-- Header -->
          <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
            <h3 class="text-sm font-bold">{$t('onboardingPage.enableFullAccess')}</h3>
            <button onclick={cancelFullAccessEnable} class="w-7 h-7 flex items-center justify-center rounded-lg hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent">
              <X class="w-4 h-4" />
            </button>
          </div>

          <div class="px-5 py-4 space-y-4">
            <div class="flex items-start gap-3">
              <AlertTriangle class="w-5 h-5 text-warning shrink-0 mt-0.5" />
              <p class="text-xs text-base-content leading-relaxed">
                {$t('onboardingPage.enableFullAccessDesc')}
              </p>
            </div>

            <div class="rounded-xl bg-error/10 border border-error/20 p-4">
              <p class="text-xs font-semibold text-error mb-2">{$t('settingsPermissions.risks')}</p>
              <ul class="text-xs text-base-content/70 space-y-1 list-disc list-inside">
                <li>{$t('settingsPermissions.risk1')}</li>
                <li>{$t('settingsPermissions.risk2')}</li>
                <li>{$t('settingsPermissions.risk3')}</li>
                <li>{$t('settingsPermissions.risk4')}</li>
              </ul>
            </div>

            <div class="rounded-xl bg-base-200 p-4 max-h-28 overflow-y-auto">
              <p class="text-xs text-base-content/70 leading-relaxed">
                {$t('onboardingPage.fullAccessDisclaimer')}
              </p>
            </div>

            <label class="flex items-center gap-3 cursor-pointer">
              <input type="checkbox" class="checkbox checkbox-sm checkbox-warning" bind:checked={termsAccepted} />
              <span class="text-xs font-medium">{$t('settingsPermissions.acceptRisks')}</span>
            </label>

            <div>
              <label class="block text-xs font-medium mb-1.5" for="onboard-confirm-enable">{$t('settingsPermissions.typeEnable')}</label>
              <input
                id="onboard-confirm-enable"
                type="text"
                class="w-full h-9 rounded-lg bg-base-200 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
                placeholder={$t('settingsPermissions.enableWord')}
                bind:value={confirmText}
                onkeydown={(e) => { if (e.key === 'Enter') confirmFullAccessEnable(); }}
              />
            </div>
          </div>

          <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
            <button
              onclick={cancelFullAccessEnable}
              class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
            >
              {$t('common.cancel')}
            </button>
            <button
              onclick={confirmFullAccessEnable}
              disabled={!canConfirm}
              class="px-4 py-2 rounded-lg bg-error text-error-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none disabled:opacity-30 disabled:cursor-not-allowed"
            >
              {$t('onboardingPage.enableFullAccess')}
            </button>
          </div>
        </div>
      </div>
    {/if}

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={() => (step = 2)}
        class="inline-flex items-center gap-2 px-3 py-3 rounded-xl text-sm font-medium text-base-content/60 cursor-pointer hover:text-base-content transition-colors border-none bg-transparent"
      >
        <ArrowLeft class="w-4 h-4" /> {$t('common.back')}
      </button>
      <button
        onclick={savePermissions}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
      >
        {$t('common.continue')} <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>

{:else if step === 4}
  <!-- Done -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-success/10 flex items-center justify-center mx-auto mb-5">
      <Check class="w-7 h-7 text-success" />
    </div>
    <h2 class="text-2xl font-bold mb-2">{$t('onboarding.complete.title')}</h2>
    <p class="text-xs text-base-content/50 mb-6">{$t('onboardingPage.doneDesc')}</p>

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={finish}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
      >
        {$t('onboardingPage.openNebo')} <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>
{/if}
