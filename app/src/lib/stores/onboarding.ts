import { writable, get } from 'svelte/store';
import { storage } from '$lib/storage';
import { backendHealth } from '$lib/api/base';
import { logger } from '$lib/monitoring';

// Onboarding state tracks whether setup is complete.
// On init, we check localStorage for a cached value, then verify with the backend.
const stored = storage.get('nebo-onboarding-complete');
export const onboardingComplete = writable(stored === 'true');
export const onboardingChecked = writable(false);

// Backend readiness — false until we get a successful response from the backend.
export const backendReady = writable(false);
export const backendChecking = writable(false);

// Sync to localStorage whenever it changes
onboardingComplete.subscribe(v => {
  storage.set('nebo-onboarding-complete', String(v));
});

let pollTimer: ReturnType<typeof setTimeout> | null = null;

async function tryBackendHealth(): Promise<boolean> {
  return (await backendHealth()) !== null;
}

function startPolling() {
  if (pollTimer) return;
  const poll = async () => {
    if (await tryBackendHealth()) {
      pollTimer = null;
      backendReady.set(true);
      backendChecking.set(false);
      checkOnboardingStatus();
    } else {
      pollTimer = setTimeout(poll, 2000);
    }
  };
  pollTimer = setTimeout(poll, 2000);
}

export function retryBackendConnection() {
  backendChecking.set(true);
  tryBackendHealth().then(ok => {
    if (ok) {
      backendReady.set(true);
      backendChecking.set(false);
      checkOnboardingStatus();
    } else {
      startPolling();
    }
  });
}

/**
 * Check onboarding status from the backend.
 * If backend is unreachable, starts polling and shows loading screen.
 */
export async function checkOnboardingStatus(): Promise<boolean> {
  try {
    const { status } = await import('$lib/api/nebo');
    const statusResp = await status() as { setupComplete?: boolean };
    const complete = !!statusResp?.setupComplete;
    if (!complete) {
      // Backend reports a fresh profile (data dir was wiped / first ever run), but
      // browser storage survives that wipe. Clear THIS install's stale UI prefs
      // (base-scoped — never another bot's) so "fresh" is truly fresh.
      storage.remove('nebo:sidebar-collapsed');
    }
    onboardingComplete.set(complete);
    onboardingChecked.set(true);
    backendReady.set(true);
    return complete;
  } catch {
    logger.warn('Could not reach backend for onboarding status');
    backendReady.set(false);
    startPolling();
    return false;
  }
}

/**
 * Mark onboarding as complete on the backend and locally.
 */
export async function completeOnboarding(): Promise<void> {
  // Update local state immediately for responsive UI
  onboardingComplete.set(true);

  try {
    const { userUpdatePreferences, complete } = await import('$lib/api/nebo');
    await Promise.all([
      userUpdatePreferences({ onboardingCompleted: true }),
      complete()
    ]);
    logger.info('Onboarding completed on backend');
  } catch (err) {
    logger.error('Failed to save onboarding completion to backend', err);
    // Local state is already set — user can proceed.
    // Backend will re-check on next load.
  }
}

/**
 * The guided first-run chat, once setup is complete: what the shell shows
 * for the server's step (`GET /onboarding`). `intro` is the three intro
 * screens; every chat step (name … set) is the assistant's own chat, already
 * under way; `done` (and an older bot without the endpoint) is nothing.
 */
export type FirstRunView = 'intro' | 'chat' | 'done';

const CHAT_STEPS = new Set(['name', 'focus', 'connect', 'plate', 'day', 'daily', 'hire', 'set']);

export function firstRunView(step: string | null | undefined): FirstRunView {
  if (step === 'intro') return 'intro';
  return step && CHAT_STEPS.has(step) ? 'chat' : 'done';
}

/**
 * Read where the first-run chat is, once, when the shell mounts. A chat
 * step that was cut short (an app closed mid-conversation) resumes:
 * `POST /onboarding/start` is idempotent and never starts a second turn
 * while one runs. A bot that cannot answer (an older server: a 404, or the
 * app page in place of the endpoint) is done.
 */
export async function checkFirstRun(): Promise<FirstRunView> {
  try {
    const { getOnboarding, startOnboarding } = await import('$lib/api/nebo');
    const view = firstRunView((await getOnboarding())?.step);
    if (view === 'chat') await startOnboarding();
    return view;
  } catch (err) {
    logger.warn('First-run status unavailable', { error: String(err) });
    return 'done';
  }
}

/**
 * Leave the intro screens (Get started or Skip): the server moves to the
 * first chat step and the assistant starts talking. Returns the assistant's
 * thread to open, or null when there is none (finished, or an older bot).
 */
export async function startFirstRun(): Promise<string | null> {
  try {
    const { startOnboarding } = await import('$lib/api/nebo');
    return (await startOnboarding())?.threadId ?? null;
  } catch (err) {
    logger.warn('First-run start unavailable', { error: String(err) });
    return null;
  }
}
