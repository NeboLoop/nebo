<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  import { completeOnboarding } from '$lib/stores/onboarding';
  import { logger } from '$lib/monitoring';
  import * as api from '$lib/api/nebo';
  import { neboLoopOAuthStartWithJanus, neboLoopOAuthStatus } from '$lib/api/index';
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
  let autonomous = $state(false);
  let showApprovalPreview = $state(false);
  let showEnableModal = $state(false);
  let termsAccepted = $state(false);
  let confirmText = $state('');
  const canConfirm = $derived(termsAccepted && confirmText === 'ENABLE');
  let selectedLocale = $state('en');

  // Language picker hidden for v0.10.0. Set SHOW_LANGUAGE_STEP=true to re-enable.
  const SHOW_LANGUAGE_STEP = false;

  // NeboLoop OAuth state
  let neboLoopLoading = $state(false);
  let neboLoopConnected = $state(false);
  let neboLoopEmail = $state('');
  let neboLoopError = $state('');
  let neboLoopPendingState = $state('');
  let neboLoopPollInterval: ReturnType<typeof setInterval> | null = null;
  let neboLoopTimeout: ReturnType<typeof setTimeout> | null = null;

  const CAPABILITY_LABELS: Record<string, { label: string; desc: string }> = {
    chat: { label: 'Chat', desc: 'Respond to messages and conversations' },
    file: { label: 'File Access', desc: 'Read and write files on your system' },
    shell: { label: 'Shell Commands', desc: 'Execute terminal commands' },
    web: { label: 'Web Access', desc: 'Make HTTP requests and browse the web' },
    contacts: { label: 'Contacts', desc: 'Access your contacts and address book' },
    desktop: { label: 'Desktop', desc: 'Control mouse, keyboard, and windows' },
    media: { label: 'Media', desc: 'Access camera, microphone, and screen' },
    system: { label: 'System', desc: 'Access system information and settings' },
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
      const allKeys = new Set([...Object.keys(CAPABILITY_LABELS), ...Object.keys(permObj)]);
      permissions = Array.from(allKeys).map(key => ({
        key,
        label: CAPABILITY_LABELS[key]?.label || key.charAt(0).toUpperCase() + key.slice(1),
        desc: CAPABILITY_LABELS[key]?.desc || '',
        enabled: permObj[key] ?? true,
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

  // NeboLoop OAuth flow
  function cleanupNeboLoopOAuth() {
    if (neboLoopPollInterval) {
      clearInterval(neboLoopPollInterval);
      neboLoopPollInterval = null;
    }
    if (neboLoopTimeout) {
      clearTimeout(neboLoopTimeout);
      neboLoopTimeout = null;
    }
  }

  async function connectNeboLoop() {
    neboLoopLoading = true;
    neboLoopError = '';

    try {
      // Start OAuth — backend opens browser with OAuth URL
      const result = await neboLoopOAuthStartWithJanus(true);
      neboLoopPendingState = result.state;

      // Set 3-minute timeout
      neboLoopTimeout = setTimeout(() => {
        cleanupNeboLoopOAuth();
        neboLoopLoading = false;
        neboLoopError = 'Connection timed out. Please try again.';
      }, 180_000);

      // Poll every 2 seconds for completion
      neboLoopPollInterval = setInterval(async () => {
        try {
          const status = await neboLoopOAuthStatus(neboLoopPendingState);
          if (status?.status === 'complete') {
            cleanupNeboLoopOAuth();
            neboLoopConnected = true;
            neboLoopEmail = status.email ?? '';
            neboLoopLoading = false;
            logger.info('NeboLoop OAuth completed');
          } else if (status?.status === 'error') {
            cleanupNeboLoopOAuth();
            neboLoopLoading = false;
            neboLoopError = status.error || 'OAuth failed. Please try again.';
          } else if (status?.status === 'expired') {
            cleanupNeboLoopOAuth();
            neboLoopLoading = false;
            neboLoopError = 'OAuth session expired. Please try again.';
          }
          // 'pending' — keep polling
        } catch {
          // Network error during poll — keep trying silently
        }
      }, 2000);
    } catch (err) {
      neboLoopLoading = false;
      neboLoopError = err instanceof Error ? err.message : 'Failed to start OAuth flow';
      logger.error('NeboLoop OAuth start failed', err);
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

<svelte:head><title>Welcome to Nebo</title></svelte:head>

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
    <h2 class="text-2xl font-bold mb-2">Welcome to Nebo</h2>
    <p class="text-xs text-base-content/50 mb-6 max-w-sm mx-auto">Your AI agent team, running locally on your machine. Let's get you set up in under a minute.</p>

    <div class="max-w-sm mx-auto rounded-xl border border-base-300 bg-base-200/30 p-4 mb-8 text-left">
      <div class="flex items-start gap-3">
        <Shield class="w-5 h-5 text-warning shrink-0 mt-0.5" />
        <div class="flex-1 min-w-0">
          <div class="text-sm font-medium mb-1">Terms & Privacy</div>
          <p class="text-xs text-base-content/70 leading-relaxed mb-3">
            Nebo runs AI agents on your machine. Agents can read files, execute commands, and access the web based on the permissions you grant. By continuing, you agree to the <a href="/terms" class="text-primary underline">Terms of Service</a> and <a href="/privacy" class="text-primary underline">Privacy Policy</a>.
          </p>
          <label class="flex items-center gap-2 cursor-pointer">
            <input type="checkbox" class="checkbox checkbox-sm checkbox-primary" bind:checked={tcAccepted} />
            <span class="text-sm font-medium">I accept the Terms & Conditions</span>
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
        Get Started <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>

{:else if step === 1 && SHOW_LANGUAGE_STEP}
  <!-- Language -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-primary/10 flex items-center justify-center mx-auto mb-5">
      <Globe class="w-7 h-7 text-primary" />
    </div>
    <h2 class="text-2xl font-bold mb-2">Choose your language</h2>
    <p class="text-xs text-base-content/50 mb-6">You can change this later in Settings.</p>

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
        <ArrowLeft class="w-4 h-4" /> Back
      </button>
      <button
        onclick={saveLocale}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
      >
        Continue <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>

{:else if step === 2}
  <!-- Connect NeboLoop -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-primary/10 flex items-center justify-center mx-auto mb-5">
      <Link class="w-7 h-7 text-primary" />
    </div>
    <h2 class="text-2xl font-bold mb-2">Connect NeboLoop</h2>
    <p class="text-xs text-base-content/50 mb-6">Connect your NeboLoop account to access the marketplace, billing, and the Janus AI gateway.</p>

    <div class="mb-8">
      {#if neboLoopConnected}
        <div class="p-4 rounded-xl border border-success/30 bg-success/5 max-w-sm mx-auto">
          <div class="flex items-center justify-center gap-2">
            <Check class="w-5 h-5 text-success" />
            <span class="text-sm font-semibold text-success">Connected{neboLoopEmail ? ` as ${neboLoopEmail}` : ''}</span>
          </div>
        </div>
      {:else if neboLoopLoading}
        <div class="max-w-sm mx-auto">
          <button
            disabled
            class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold border-none opacity-80 mb-3"
          >
            <span class="loading loading-spinner loading-sm"></span> Waiting for authorization...
          </button>
          <p class="text-xs text-base-content/50">A browser window has been opened. Complete the sign-in there, then return here.</p>
        </div>
      {:else}
        <div class="max-w-sm mx-auto">
          <button
            onclick={connectNeboLoop}
            class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
          >
            Connect with NeboLoop
          </button>
          {#if neboLoopError}
            <p class="text-xs text-error mt-3">{neboLoopError}</p>
          {/if}
        </div>
      {/if}
    </div>

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={() => { cleanupNeboLoopOAuth(); step = SHOW_LANGUAGE_STEP ? 1 : 0; }}
        class="inline-flex items-center gap-2 px-3 py-3 rounded-xl text-sm font-medium text-base-content/60 cursor-pointer hover:text-base-content transition-colors border-none bg-transparent"
      >
        <ArrowLeft class="w-4 h-4" /> Back
      </button>
      {#if neboLoopConnected}
        <button
          onclick={() => (step = 3)}
          class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
        >
          Continue <ArrowRight class="w-4 h-4" />
        </button>
      {:else}
        <button
          onclick={() => { cleanupNeboLoopOAuth(); step = 3; }}
          class="inline-flex items-center gap-2 px-3 py-3 rounded-xl text-sm font-medium text-base-content/60 cursor-pointer hover:text-base-content transition-colors border-none bg-transparent"
        >
          Skip for now <ArrowRight class="w-4 h-4" />
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
    <h2 class="text-2xl font-bold mb-2">Permissions</h2>
    <p class="text-xs text-base-content/50 mb-6">Choose what your agents can access. You can change these later in Settings.</p>

    <!-- Autonomous mode -->
    <div class="flex items-center justify-between p-4 rounded-xl border border-base-300 mb-5 max-w-md mx-auto text-left">
      <div>
        <div class="text-sm font-semibold flex items-center gap-2">
          {#if autonomous}<AlertTriangle class="w-4 h-4 text-warning" />{/if}
          Autonomous Mode
        </div>
        <div class="text-xs text-base-content/70">The agent will execute all tools without asking for permission.</div>
      </div>
      <input
        type="checkbox"
        class="toggle toggle-sm toggle-primary shrink-0 ml-4"
        checked={autonomous}
        onchange={() => { if (!autonomous) { showEnableModal = true; } else { autonomous = false; } }}
      />
    </div>

    {#if !autonomous}
      <div class="divide-y divide-base-content/10 mb-5 max-w-md mx-auto text-left">
        {#each permissions as perm, i}
          <div class="flex items-center justify-between py-3">
            <div>
              <div class="text-sm font-medium">{perm.label}</div>
              <div class="text-xs text-base-content/70">{perm.desc}</div>
            </div>
            {#if perm.locked}
              <span class="text-xs text-base-content/50 font-mono shrink-0 ml-4">Always on</span>
            {:else}
              <input type="checkbox" class="toggle toggle-sm toggle-primary shrink-0 ml-4" bind:checked={capStates[i]} />
            {/if}
          </div>
        {/each}
      </div>
    {:else}
      <div class="rounded-xl bg-warning/10 border border-warning/20 px-4 py-3 mb-5 max-w-md mx-auto text-left">
        <p class="text-xs text-warning font-medium">Autonomous Mode is active</p>
        <p class="text-xs text-base-content/70 mt-0.5">All approval prompts are bypassed. Make sure you trust the prompts you're sending.</p>
      </div>
    {/if}

    <!-- Approval dialog preview -->
    <button
      onclick={() => showApprovalPreview = true}
      class="inline-flex items-center gap-2 px-3 py-2 rounded-lg text-xs font-medium text-base-content/60 cursor-pointer hover:text-base-content hover:bg-base-200/50 transition-colors border-none bg-transparent mb-8"
    >
      <Eye class="w-3.5 h-3.5" /> Preview what agents see when asking for permission
    </button>

    <ApprovalModal
      bind:show={showApprovalPreview}
      agent="Research Agent"
      actionType="shell_command"
      actionDetail="curl -s https://api.example.com/data | jq '.results[]'"
      actionKey="preview"
    />

    <!-- Autonomous Mode Activation Modal -->
    {#if showEnableModal}
      <div class="fixed inset-0 z-[80] flex items-center justify-center p-4" role="dialog" aria-modal="true">
        <div class="absolute inset-0 bg-black/60 backdrop-blur-sm" role="button" tabindex="-1" aria-label="Close" onclick={() => { showEnableModal = false; termsAccepted = false; confirmText = ''; }} onkeydown={(e) => { if (e.key === 'Escape') { showEnableModal = false; termsAccepted = false; confirmText = ''; } }}></div>
        <div class="relative w-full max-w-lg rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden text-left">
          <!-- Header -->
          <div class="flex items-center justify-between px-5 py-4 border-b border-base-content/10">
            <h3 class="text-sm font-bold">Enable Autonomous Mode</h3>
            <button onclick={() => { showEnableModal = false; termsAccepted = false; confirmText = ''; }} class="w-7 h-7 flex items-center justify-center rounded-lg hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent">
              <X class="w-4 h-4" />
            </button>
          </div>

          <div class="px-5 py-4 space-y-4">
            <div class="flex items-start gap-3">
              <AlertTriangle class="w-5 h-5 text-warning shrink-0 mt-0.5" />
              <p class="text-xs text-base-content leading-relaxed">
                This will allow your agent to execute all tools — including shell commands, file modifications, and network requests — without asking for permission.
              </p>
            </div>

            <div class="rounded-xl bg-error/10 border border-error/20 p-4">
              <p class="text-xs font-semibold text-error mb-2">Risks include:</p>
              <ul class="text-xs text-base-content/70 space-y-1 list-disc list-inside">
                <li>The agent may modify or delete files on your system</li>
                <li>The agent may execute arbitrary shell commands</li>
                <li>The agent may make network requests and access external services</li>
                <li>You are solely responsible for any actions taken by the agent</li>
              </ul>
            </div>

            <div class="rounded-xl bg-base-200 p-4 max-h-28 overflow-y-auto">
              <p class="text-xs text-base-content/70 leading-relaxed">
                By enabling autonomous mode, you acknowledge that Nebo Labs, Inc. shall not be liable for any damages, losses, or consequences arising from the autonomous execution of tools by the agent. You accept full responsibility for all actions taken by the agent while autonomous mode is enabled.
              </p>
            </div>

            <label class="flex items-center gap-3 cursor-pointer">
              <input type="checkbox" class="checkbox checkbox-sm checkbox-warning" bind:checked={termsAccepted} />
              <span class="text-xs font-medium">I understand the risks and accept full responsibility</span>
            </label>

            <div>
              <label class="block text-xs font-medium mb-1.5" for="onboard-confirm-enable">Type ENABLE to confirm</label>
              <input
                id="onboard-confirm-enable"
                type="text"
                class="w-full h-9 rounded-lg bg-base-200 border border-base-content/10 px-3 text-sm focus:outline-none focus:border-primary/50 transition-colors"
                placeholder="ENABLE"
                bind:value={confirmText}
                onkeydown={(e) => { if (e.key === 'Enter' && canConfirm) { autonomous = true; showEnableModal = false; termsAccepted = false; confirmText = ''; } }}
              />
            </div>
          </div>

          <div class="flex items-center justify-end gap-2 px-5 py-4 border-t border-base-content/10">
            <button
              onclick={() => { showEnableModal = false; termsAccepted = false; confirmText = ''; }}
              class="px-4 py-2 rounded-lg border border-base-content/10 text-sm font-medium cursor-pointer hover:bg-base-200 transition-colors bg-transparent"
            >
              Cancel
            </button>
            <button
              onclick={() => { if (canConfirm) { autonomous = true; showEnableModal = false; termsAccepted = false; confirmText = ''; } }}
              disabled={!canConfirm}
              class="px-4 py-2 rounded-lg bg-error text-error-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none disabled:opacity-30 disabled:cursor-not-allowed"
            >
              Enable Autonomous Mode
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
        <ArrowLeft class="w-4 h-4" /> Back
      </button>
      <button
        onclick={savePermissions}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
      >
        Continue <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>

{:else if step === 4}
  <!-- Done -->
  <div class="text-center">
    <div class="w-14 h-14 rounded-2xl bg-success/10 flex items-center justify-center mx-auto mb-5">
      <Check class="w-7 h-7 text-success" />
    </div>
    <h2 class="text-2xl font-bold mb-2">You're all set!</h2>
    <p class="text-xs text-base-content/50 mb-6">Nebo is ready. Your agent team is standing by.</p>

    <div class="flex items-center justify-center gap-3">
      <button
        onclick={finish}
        class="inline-flex items-center gap-2 px-6 py-3 rounded-xl bg-primary text-primary-content text-sm font-bold cursor-pointer hover:brightness-110 transition-all border-none"
      >
        Open Nebo <ArrowRight class="w-4 h-4" />
      </button>
    </div>
  </div>
{/if}
