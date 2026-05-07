import { writable, get } from 'svelte/store';
import { logger } from '$lib/monitoring';

// Onboarding state tracks whether setup is complete.
// On init, we check localStorage for a cached value, then verify with the backend.
const stored = typeof localStorage !== 'undefined' ? localStorage.getItem('nebo-onboarding-complete') : null;
export const onboardingComplete = writable(stored === 'true');
export const onboardingChecked = writable(false);

// Sync to localStorage whenever it changes
onboardingComplete.subscribe(v => {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('nebo-onboarding-complete', String(v));
  }
});

/**
 * Check onboarding status from the backend.
 * Falls back to localStorage if backend is unreachable.
 */
export async function checkOnboardingStatus(): Promise<boolean> {
  try {
    const { getUserProfile } = await import('$lib/api/nebo');
    const response = await getUserProfile();
    const complete = !!response.profile?.onboardingCompleted;
    onboardingComplete.set(complete);
    onboardingChecked.set(true);
    return complete;
  } catch {
    // Backend unreachable — try setup status endpoint
    try {
      const { setupStatus } = await import('$lib/api/nebo');
      const statusResp = await setupStatus();
      const complete = !!statusResp.setupComplete;
      onboardingComplete.set(complete);
      onboardingChecked.set(true);
      return complete;
    } catch {
      // Both endpoints failed — trust localStorage cache
      logger.warn('Could not reach backend for onboarding status, using cached value');
      onboardingChecked.set(true);
      return get(onboardingComplete);
    }
  }
}

/**
 * Mark onboarding as complete on the backend and locally.
 */
export async function completeOnboarding(): Promise<void> {
  // Update local state immediately for responsive UI
  onboardingComplete.set(true);

  try {
    const { updateUserProfile, completeSetup } = await import('$lib/api/nebo');
    await Promise.all([
      updateUserProfile({ onboardingCompleted: true }),
      completeSetup()
    ]);
    logger.info('Onboarding completed on backend');
  } catch (err) {
    logger.error('Failed to save onboarding completion to backend', err);
    // Local state is already set — user can proceed.
    // Backend will re-check on next load.
  }
}
