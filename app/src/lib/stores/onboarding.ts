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
      storage.remove('nebo:tour-pending');
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
