import { writable } from 'svelte/store';

// Persisted auto-approved actions
const stored = typeof localStorage !== 'undefined' ? localStorage.getItem('nebo-permissions') : null;
const initial: string[] = stored ? JSON.parse(stored) : [];

export const autoApproved = writable<string[]>(initial);

autoApproved.subscribe(v => {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('nebo-permissions', JSON.stringify(v));
  }
});

export function approveAlways(actionKey: string) {
  autoApproved.update(list => [...list, actionKey]);
}

export function isAutoApproved(actionKey: string): boolean {
  let result = false;
  autoApproved.subscribe(list => { result = list.includes(actionKey); })();
  return result;
}
