import { writable } from 'svelte/store';
import { browser } from '$app/environment';

// User-facing theme choice: 'light' | 'dark' | 'system'.
// 'system' follows the OS prefers-color-scheme media query.
export type ThemeMode = 'light' | 'dark' | 'system';

// Underlying DaisyUI theme id that gets applied to <html data-theme>.
// 'light' → 'clean' (our custom light), 'dark' → 'dark', 'system' → resolved at runtime.
const LIGHT_THEME = 'clean';
const DARK_THEME = 'dark';

function resolveSystemTheme(): string {
  if (!browser) return LIGHT_THEME;
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? DARK_THEME : LIGHT_THEME;
}

function loadMode(): ThemeMode {
  if (!browser) return 'system';
  const saved = localStorage.getItem('nebo-theme-mode');
  if (saved === 'light' || saved === 'dark' || saved === 'system') return saved;
  // Migrate legacy 'nebo-theme' value
  const legacy = localStorage.getItem('nebo-theme');
  if (legacy === 'dark') return 'dark';
  if (legacy === 'clean' || legacy === 'light') return 'light';
  return 'system';
}

export const themeMode = writable<ThemeMode>(loadMode());

// Applied daisy theme — derived from mode + system preference.
export const theme = writable<string>(LIGHT_THEME);

function applyTheme(mode: ThemeMode) {
  const applied = mode === 'system' ? resolveSystemTheme() : (mode === 'dark' ? DARK_THEME : LIGHT_THEME);
  theme.set(applied);
  if (browser) {
    document.documentElement.setAttribute('data-theme', applied);
  }
}

if (browser) {
  // Apply on load
  applyTheme(loadMode());

  // Persist + re-apply whenever mode changes
  themeMode.subscribe((mode) => {
    localStorage.setItem('nebo-theme-mode', mode);
    applyTheme(mode);
  });

  // React to OS theme changes only while in 'system' mode
  const mql = window.matchMedia('(prefers-color-scheme: dark)');
  mql.addEventListener('change', () => {
    const current = loadMode();
    if (current === 'system') applyTheme('system');
  });
}
