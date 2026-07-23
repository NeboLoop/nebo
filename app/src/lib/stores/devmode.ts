import { writable } from 'svelte/store';
import { storage } from '$lib/storage';

// Persisted dev mode toggle — gates advanced settings like Providers, Routing, Secrets
const stored = storage.get('nebo-devmode');
export const devMode = writable(stored === 'true');

devMode.subscribe(v => {
  {
    storage.set('nebo-devmode', String(v));
  }
});
