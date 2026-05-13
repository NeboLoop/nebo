import { writable } from 'svelte/store';

// Persisted dev mode toggle — gates advanced settings like Providers, Routing, Secrets
const stored = typeof localStorage !== 'undefined' ? localStorage.getItem('nebo-devmode') : null;
export const devMode = writable(stored === 'true');

devMode.subscribe(v => {
  if (typeof localStorage !== 'undefined') {
    localStorage.setItem('nebo-devmode', String(v));
  }
});
