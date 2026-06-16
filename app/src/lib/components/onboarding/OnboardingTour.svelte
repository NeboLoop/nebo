<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { dispatchInstallStart } from '$lib/marketplace/installCodes';

  // Proactive first-run onboarding: account-type question → scripted welcome →
  // spotlight tour → one-tap setup. Deterministic (no LLM). Fires once; the
  // `nebo:tour-done` flag (localStorage) keeps it from ever repeating.
  const DONE_KEY = 'nebo:tour-done';

  type Phase = 'hidden' | 'account' | 'welcome' | 'tour' | 'finale';
  let phase = $state<Phase>('hidden');
  let stepIndex = $state(0);
  let accountType = $state<'personal' | 'business'>('personal');
  let rect = $state<{ top: number; left: number; width: number; height: number } | null>(null);

  // First-run capabilities to install in one tap (canonical handle_code pathway).
  const SETUP_CODES = ['PLUG-BHVY-A96N', 'SKIL-VQTF-WV8E', 'SKIL-TV64-VHQ4']; // Office, Design, NeboAI

  type Step = { target: string; title: string; body: { personal: string; business: string } };
  const STEPS: Step[] = [
    {
      target: '[data-tour="chat"]',
      title: 'This is where we talk',
      body: {
        personal: 'Ask me anything, or just tell me what to do — I take it from there.',
        business: 'Ask me anything, or just tell me what to do — I take it from there.',
      },
    },
    {
      target: '[data-tour="agents"]',
      title: 'Your companions live here',
      body: {
        personal: "I'm Nebo. You can add specialists anytime — each is great at a different job.",
        business: "I'm Nebo. Add specialists anytime — a Chief of Staff, a researcher, and more.",
      },
    },
    {
      target: '[data-tour="work"]',
      title: 'Your work shows up here',
      body: {
        personal: 'Notes, lists, and documents I make for you open in this panel.',
        business: 'Documents, spreadsheets, and decks I make for you open in this panel.',
      },
    },
    {
      target: '[data-tour="schedule"]',
      title: 'Put things on autopilot',
      body: {
        personal: 'Set reminders and routines — appointments, bills, a morning rundown.',
        business: 'Automate the recurring work — morning briefings, follow-ups, reports.',
      },
    },
    {
      target: '[data-tour="marketplace"]',
      title: 'Add new abilities',
      body: {
        personal: 'Browse skills and tools — document editing, design, and more.',
        business: 'Browse skills and tools — document editing, design, a Chief of Staff, and more.',
      },
    },
    {
      target: '[data-tour="search"]',
      title: 'Jump anywhere fast',
      body: {
        personal: 'Press ⌘K to search or run any command in a flash.',
        business: 'Press ⌘K to search or run any command in a flash.',
      },
    },
  ];

  // Only tour anchors that are actually mounted — computed when the tour starts.
  let activeSteps = $state<Step[]>(STEPS);
  const step = $derived(activeSteps[stepIndex]);
  const welcomeNoun = $derived(accountType === 'business' ? 'your business' : 'your life');

  function shouldRun(): boolean {
    try {
      return typeof localStorage !== 'undefined' && localStorage.getItem(DONE_KEY) !== '1';
    } catch {
      return false;
    }
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
    try {
      localStorage.setItem(DONE_KEY, '1');
    } catch {
      /* ignore */
    }
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
      <h2 class="text-xl font-bold mb-1">Hi, I'm Nebo Desktop.</h2>
      <p class="text-sm text-base-content/70 mb-6">First — who am I mostly here to help?</p>
      <div class="grid grid-cols-2 gap-3">
        <button onclick={() => pickAccount('personal')} class="rounded-xl border border-base-300 bg-base-100 hover:border-primary hover:bg-primary/5 transition-colors p-4 cursor-pointer text-left">
          <div class="text-sm font-semibold mb-0.5">Personal</div>
          <div class="text-xs text-base-content/60">Organize and automate your life.</div>
        </button>
        <button onclick={() => pickAccount('business')} class="rounded-xl border border-base-300 bg-base-100 hover:border-primary hover:bg-primary/5 transition-colors p-4 cursor-pointer text-left">
          <div class="text-sm font-semibold mb-0.5">Business</div>
          <div class="text-xs text-base-content/60">Organize and automate your work.</div>
        </button>
      </div>
    </div>
  </div>
{:else if phase === 'welcome'}
  <div class="fixed inset-0 z-[90] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
    <div class="w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-7 text-center">
      <div class="w-12 h-12 rounded-xl bg-primary text-primary-content flex items-center justify-center font-mono text-xl font-bold mx-auto mb-4">N</div>
      <h2 class="text-xl font-bold mb-2">Welcome aboard.</h2>
      <p class="text-sm text-base-content/80 mb-6 leading-relaxed">
        I live on your computer and help you organize and automate {welcomeNoun}. Let me give you a quick tour so you know where everything is.
      </p>
      <div class="flex items-center justify-center gap-2">
        <button onclick={finish} class="px-4 py-2 rounded-lg border border-base-300 text-sm font-medium hover:bg-base-200 transition-colors cursor-pointer bg-transparent">Skip</button>
        <button onclick={startTour} class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all cursor-pointer border-none">Start the tour</button>
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
      <div class="text-sm font-semibold mb-1">{step.title}</div>
      <div class="text-xs text-base-content/70 leading-relaxed mb-3">{step.body[accountType]}</div>
      <div class="flex items-center justify-between">
        <span class="text-xs text-base-content/50 font-mono">{stepIndex + 1} / {activeSteps.length}</span>
        <div class="flex items-center gap-1.5">
          <button onclick={finish} class="px-2.5 py-1 rounded-md text-xs font-medium text-base-content/60 hover:bg-base-200 transition-colors cursor-pointer bg-transparent border-none">Skip</button>
          {#if stepIndex > 0}
            <button onclick={back} class="px-2.5 py-1 rounded-md text-xs font-medium border border-base-300 hover:bg-base-200 transition-colors cursor-pointer bg-transparent">Back</button>
          {/if}
          <button onclick={next} class="px-3 py-1 rounded-md text-xs font-bold bg-primary text-primary-content hover:brightness-110 transition-all cursor-pointer border-none">
            {stepIndex === activeSteps.length - 1 ? 'Finish' : 'Next'}
          </button>
        </div>
      </div>
    </div>
  </div>
{:else if phase === 'finale'}
  <div class="fixed inset-0 z-[90] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
    <div class="w-full max-w-md rounded-2xl bg-base-100 border border-base-300 shadow-2xl p-7 text-center">
      <div class="w-12 h-12 rounded-xl bg-primary text-primary-content flex items-center justify-center text-xl mx-auto mb-4">✨</div>
      <h2 class="text-xl font-bold mb-2">That's the lay of the land.</h2>
      <p class="text-sm text-base-content/80 mb-6 leading-relaxed">
        Want me to set up your core tools now? I'll add document editing (Word, Excel, PowerPoint, PDF), design, and publishing — so you can start making real things right away.
      </p>
      <div class="flex items-center justify-center gap-2">
        <button onclick={finish} class="px-4 py-2 rounded-lg border border-base-300 text-sm font-medium hover:bg-base-200 transition-colors cursor-pointer bg-transparent">Maybe later</button>
        <button onclick={runSetup} class="px-4 py-2 rounded-lg bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all cursor-pointer border-none">Set me up</button>
      </div>
    </div>
  </div>
{/if}
