<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount, onDestroy } from 'svelte';
  import { dispatchInstallStart } from '$lib/marketplace/installCodes';
  import { storage } from '$lib/storage';

  // Proactive first-run onboarding: account-type question → scripted welcome →
  // spotlight tour → one-tap setup. Deterministic (no LLM). Runs ONLY on the
  // one-shot handoff flag the onboarding wizard sets when it completes — never
  // on "flag missing in this browser", which re-showed the tour on every new
  // browser (and, tunnel-served, whenever ANOTHER bot cleared shared storage).
  const PENDING_KEY = 'nebo:tour-pending';

  type Phase = 'hidden' | 'account' | 'welcome' | 'tour' | 'finale';
  let phase = $state<Phase>('hidden');
  let stepIndex = $state(0);
  let accountType = $state<'personal' | 'business'>('personal');
  let rect = $state<{ top: number; left: number; width: number; height: number } | null>(null);

  // First-run capabilities to install in one tap (canonical handle_code pathway).
  const SETUP_CODES = ['PLUG-BHVY-A96N', 'SKIL-VQTF-WV8E', 'SKIL-TV64-VHQ4']; // Office, Design, NeboAI

  // Step titles/bodies are i18n key strings, resolved lazily with $t at render time.
  type Step = { target: string; title: string; body: { personal: string; business: string } };
  const STEPS: Step[] = [
    {
      target: '[data-tour="chat"]',
      title: 'onboardingTour.steps.chat.title',
      body: {
        personal: 'onboardingTour.steps.chat.bodyPersonal',
        business: 'onboardingTour.steps.chat.bodyBusiness',
      },
    },
    {
      target: '[data-tour="agents"]',
      title: 'onboardingTour.steps.agents.title',
      body: {
        personal: 'onboardingTour.steps.agents.bodyPersonal',
        business: 'onboardingTour.steps.agents.bodyBusiness',
      },
    },
    {
      target: '[data-tour="work"]',
      title: 'onboardingTour.steps.work.title',
      body: {
        personal: 'onboardingTour.steps.work.bodyPersonal',
        business: 'onboardingTour.steps.work.bodyBusiness',
      },
    },
    {
      target: '[data-tour="schedule"]',
      title: 'onboardingTour.steps.schedule.title',
      body: {
        personal: 'onboardingTour.steps.schedule.bodyPersonal',
        business: 'onboardingTour.steps.schedule.bodyBusiness',
      },
    },
    {
      target: '[data-tour="marketplace"]',
      title: 'onboardingTour.steps.marketplace.title',
      body: {
        personal: 'onboardingTour.steps.marketplace.bodyPersonal',
        business: 'onboardingTour.steps.marketplace.bodyBusiness',
      },
    },
    {
      target: '[data-tour="search"]',
      title: 'onboardingTour.steps.search.title',
      body: {
        personal: 'onboardingTour.steps.search.bodyPersonal',
        business: 'onboardingTour.steps.search.bodyBusiness',
      },
    },
  ];

  // Only tour anchors that are actually mounted — computed when the tour starts.
  let activeSteps = $state<Step[]>(STEPS);
  const step = $derived(activeSteps[stepIndex]);
  const welcomeNoun = $derived(accountType === 'business' ? $t('onboardingTour.nounBusiness') : $t('onboardingTour.nounPersonal'));

  function shouldRun(): boolean {
    return storage.get(PENDING_KEY) === '1';
  }

  function updateRect() {
    if (phase !== 'tour') return;
    const el = document.querySelector(step.target) as HTMLElement | null;
    if (!el) {
      rect = null;
      return;
    }
    const r = el.getBoundingClientRect();
    const pad = 6;
    rect = {
      top: Math.max(r.top - pad, 4),
      left: Math.max(r.left - pad, 4),
      width: r.width + pad * 2,
      height: r.height + pad * 2,
    };
  }

  async function pickAccount(type: 'personal' | 'business') {
    accountType = type;
    try {
      const api = await import('$lib/api/nebo');
      await api.userUpdateProfile({ accountType: type });
    } catch {
      /* non-blocking — the tour proceeds even if the save fails */
    }
    phase = 'welcome';
  }

  function startTour() {
    activeSteps = STEPS.filter((s) => document.querySelector(s.target));
    if (activeSteps.length === 0) {
      phase = 'finale';
      return;
    }
    phase = 'tour';
    stepIndex = 0;
    queueMicrotask(updateRect);
  }

  function next() {
    if (stepIndex < activeSteps.length - 1) {
      stepIndex += 1;
      queueMicrotask(updateRect);
    } else {
      phase = 'finale';
    }
  }

  function back() {
    if (stepIndex > 0) {
      stepIndex -= 1;
      queueMicrotask(updateRect);
    }
  }

  function finish() {
    storage.remove(PENDING_KEY);
    phase = 'hidden';
  }

  function runSetup() {
    for (const code of SETUP_CODES) dispatchInstallStart(code);
    finish();
  }

  onMount(() => {
    if (shouldRun()) phase = 'account';
    window.addEventListener('resize', updateRect);
    window.addEventListener('scroll', updateRect, true);
  });

  onDestroy(() => {
    window.removeEventListener('resize', updateRect);
    window.removeEventListener('scroll', updateRect, true);
  });

  // Tooltip sits below the highlighted element, clamped to the viewport.
  const tip = $derived.by(() => {
    if (!rect) return { top: 0, left: 0 };
    const belowRoom = window.innerHeight - (rect.top + rect.height);
    const top = belowRoom > 200 ? rect.top + rect.height + 12 : Math.max(rect.top - 188, 12);
    const left = Math.min(Math.max(rect.left, 12), window.innerWidth - 332);
    return { top, left };
  });
</script>

{#if phase === 'account'}
  <div class="fixed inset-0 z-[90] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
    <div class="w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-7 text-center">
      <div class="w-12 h-12 rounded-xl bg-primary text-primary-content flex items-center justify-center font-mono text-xl font-bold mx-auto mb-4">N</div>
      <h2 class="text-xl font-bold mb-1">{$t('onboardingTour.hiTitle')}</h2>
      <p class="text-sm text-base-content/70 mb-6">{$t('onboardingTour.accountQuestion')}</p>
      <div class="grid grid-cols-2 gap-3">
        <button onclick={() => pickAccount('personal')} class="rounded-xl border border-base-300 bg-base-100 hover:border-primary hover:bg-primary/5 transition-colors p-4 cursor-pointer text-left">
          <div class="text-sm font-semibold mb-0.5">{$t('onboardingTour.personal')}</div>
          <div class="text-xs text-base-content/60">{$t('onboardingTour.personalDesc')}</div>
        </button>
        <button onclick={() => pickAccount('business')} class="rounded-xl border border-base-300 bg-base-100 hover:border-primary hover:bg-primary/5 transition-colors p-4 cursor-pointer text-left">
          <div class="text-sm font-semibold mb-0.5">{$t('onboardingTour.business')}</div>
          <div class="text-xs text-base-content/60">{$t('onboardingTour.businessDesc')}</div>
        </button>
      </div>
    </div>
  </div>
{:else if phase === 'welcome'}
  <div class="fixed inset-0 z-[90] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
    <div class="w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-7 text-center">
      <div class="w-12 h-12 rounded-xl bg-primary text-primary-content flex items-center justify-center font-mono text-xl font-bold mx-auto mb-4">N</div>
      <h2 class="text-xl font-bold mb-2">{$t('onboardingTour.welcomeTitle')}</h2>
      <p class="text-sm text-base-content/80 mb-6 leading-relaxed">
        {$t('onboardingTour.welcomeBody', { values: { noun: welcomeNoun } })}
      </p>
      <div class="flex items-center justify-center gap-2">
        <button onclick={() => (phase = 'finale')} class="px-4 py-2 rounded-lg border border-base-300 text-sm font-medium hover:bg-base-200 transition-colors cursor-pointer bg-transparent">{$t('onboardingTour.skipTour')}</button>
        <button onclick={startTour} class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all cursor-pointer border-none">{$t('onboardingTour.startTour')}</button>
      </div>
    </div>
  </div>
{:else if phase === 'tour' && rect}
  <!-- Spotlight: a transparent hole with a huge box-shadow scrim dims everything else.
       Geometry (top/left/size) must be dynamic, so it uses style: directives; all colors
       stay in DaisyUI/Tailwind tokens. -->
  <div class="fixed inset-0 z-[90] pointer-events-none">
    <div
      class="absolute rounded-lg shadow-[0_0_0_9999px_rgba(0,0,0,0.55)] transition-all duration-200 ring-2 ring-primary"
      style:top="{rect.top}px"
      style:left="{rect.left}px"
      style:width="{rect.width}px"
      style:height="{rect.height}px"
    ></div>
    <div
      class="absolute w-[320px] rounded-xl bg-base-100 border border-base-300 shadow-2xl p-4 pointer-events-auto"
      style:top="{tip.top}px"
      style:left="{tip.left}px"
    >
      <div class="text-sm font-semibold mb-1">{$t(step.title)}</div>
      <div class="text-xs text-base-content/70 leading-relaxed mb-3">{$t(step.body[accountType])}</div>
      <div class="flex items-center justify-between">
        <span class="text-xs text-base-content/50 font-mono">{stepIndex + 1} / {activeSteps.length}</span>
        <div class="flex items-center gap-1.5">
          <button onclick={() => (phase = 'finale')} class="px-2.5 py-1 rounded-md text-xs font-medium text-base-content/60 hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none">{$t('onboardingTour.skip')}</button>
          {#if stepIndex > 0}
            <button onclick={back} class="px-2.5 py-1 rounded-md text-xs font-medium border border-base-300 hover:bg-base-200 transition-colors cursor-pointer bg-transparent">{$t('common.back')}</button>
          {/if}
          <button onclick={next} class="px-3 py-1 rounded-md text-xs font-bold bg-primary text-primary-content hover:brightness-110 transition-all cursor-pointer border-none">
            {stepIndex === activeSteps.length - 1 ? $t('onboardingTour.finish') : $t('common.next')}
          </button>
        </div>
      </div>
    </div>
  </div>
{:else if phase === 'finale'}
  <div class="fixed inset-0 z-[90] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
    <div class="w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-7 text-center">
      <div class="w-12 h-12 rounded-xl bg-primary text-primary-content flex items-center justify-center text-xl mx-auto mb-4">✨</div>
      <h2 class="text-xl font-bold mb-2">{$t('onboardingTour.finaleTitle')}</h2>
      <p class="text-sm text-base-content/80 mb-6 leading-relaxed">
        {$t('onboardingTour.finaleBody')}
      </p>
      <div class="flex items-center justify-center gap-2">
        <button onclick={finish} class="px-4 py-2 rounded-lg border border-base-300 text-sm font-medium hover:bg-base-200 transition-colors cursor-pointer bg-transparent">{$t('onboardingTour.maybeLater')}</button>
        <button onclick={runSetup} class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all cursor-pointer border-none">{$t('onboardingTour.setMeUp')}</button>
      </div>
    </div>
  </div>
{/if}
