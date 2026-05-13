import { writable } from 'svelte/store';
import { browser } from '$app/environment';

const stored = browser ? localStorage.getItem('nebo-theme') || 'nebo' : 'nebo';

export const theme = writable(stored);

if (browser) {
  theme.subscribe((value) => {
    document.documentElement.setAttribute('data-theme', value);
    localStorage.setItem('nebo-theme', value);
  });
}
