<!--
  ThemeToggle Component
  Toggle between light, dark, and system theme using DaisyUI dropdown
-->

<script lang="ts">
	import { writable } from 'svelte/store';

	// Simple theme store (you may want to connect this to a global theme store)
	const theme = writable<'light' | 'dark' | 'system'>('system');

	function setTheme(newTheme: 'light' | 'dark' | 'system') {
		theme.set(newTheme);

		// Apply theme to document
		if (newTheme === 'system') {
			const isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
			document.documentElement.classList.toggle('dark', isDark);
		} else {
			document.documentElement.classList.toggle('dark', newTheme === 'dark');
		}

		// Close dropdown by removing focus
		if (document.activeElement instanceof HTMLElement) {
			document.activeElement.blur();
		}
	}
</script>

<div class="dropdown dropdown-end">
	<div tabindex="0" role="button" class="btn btn-ghost btn-circle">
		<!-- Sun icon (light mode) -->
		<svg class="h-5 w-5 rotate-0 scale-100 transition-all dark:-rotate-90 dark:scale-0" fill="none" viewBox="0 0 24 24" stroke="currentColor">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
		</svg>
		<!-- Moon icon (dark mode) -->
		<svg class="absolute h-5 w-5 rotate-90 scale-0 transition-all dark:rotate-0 dark:scale-100" fill="none" viewBox="0 0 24 24" stroke="currentColor">
			<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
		</svg>
		<span class="sr-only">Toggle theme</span>
	</div>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<ul tabindex="0" class="dropdown-content menu bg-base-100 rounded-box z-50 w-40 p-2 shadow-lg">
		<li>
			<button type="button" onclick={() => setTheme('light')} class="flex items-center gap-2">
				<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
				</svg>
				<span>Light</span>
			</button>
		</li>
		<li>
			<button type="button" onclick={() => setTheme('dark')} class="flex items-center gap-2">
				<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
				</svg>
				<span>Dark</span>
			</button>
		</li>
		<li>
			<button type="button" onclick={() => setTheme('system')} class="flex items-center gap-2">
				<svg class="h-4 w-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9.75 17L9 20l-1 1h8l-1-1-.75-3M3 13h18M5 17h14a2 2 0 002-2V5a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
				</svg>
				<span>System</span>
			</button>
		</li>
	</ul>
</div>
