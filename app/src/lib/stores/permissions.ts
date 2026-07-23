import { writable } from 'svelte/store';
import { storage } from '$lib/storage';

// Persisted auto-approved actions
const stored = storage.get('nebo-permissions');
const initial: string[] = stored ? JSON.parse(stored) : [];

export const autoApproved = writable<string[]>(initial);

autoApproved.subscribe(v => {
  {
    storage.set('nebo-permissions', JSON.stringify(v));
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
