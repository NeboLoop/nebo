<!--
  AppNav Component
  Navigation for the authenticated app dashboard
-->

<script lang="ts">
	import { page } from '$app/stores';
	import { NeboIcon } from '$lib/components/icons';

	interface NavItem {
		label: string;
		href: string;
		icon?: string;
	}

	let {
		items = [
			{ label: 'Chat', href: '/agent', icon: 'agent' }
		] as NavItem[]
	}: {
		items?: NavItem[];
	} = $props();

	const currentPath = $derived($page.url.pathname);

	let mobileMenuOpen = $state(false);

	function toggleMobileMenu() {
		mobileMenuOpen = !mobileMenuOpen;
	}

	function closeMobileMenu() {
		mobileMenuOpen = false;
	}

	const icons: Record<string, { viewBox: string; path: string }> = {
		agent: {
			viewBox: '0 0 24 24',
			path: '<path d="M12 8V4H8"/><rect x="8" y="8" width="8" height="8" rx="1"/><path d="M12 16v4h4"/><path d="M8 12H4"/><path d="M20 12h-4"/>'
		}
	};

	function isActive(href: string): boolean {
		if (href === '/') {
			return currentPath === '/';
		}
		return currentPath.startsWith(href);
	}
</script>

<header class="layout-app-header">
	<div class="w-full mx-auto flex items-center justify-between gap-4">
		<div class="flex items-center gap-8">
			<!-- Logo -->
			<a href="/" class="flex items-center gap-2 no-underline">
				<NeboIcon class="w-12 h-12" />
				<span class="font-display text-xl font-bold text-base-content tracking-tight">Nebo</span>
			</a>

			<!-- Desktop Navigation -->
			<nav class="hidden sm:flex items-center gap-1" aria-label="Main navigation">
				{#each items as item}
					<a href={item.href} class="nav-link" class:active={isActive(item.href)}>
						{#if item.icon && icons[item.icon]}
							<svg
								class="w-[18px] h-[18px]"
								viewBox={icons[item.icon].viewBox}
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
							>
								{@html icons[item.icon].path}
							</svg>
						{/if}
						{item.label}
					</a>
				{/each}
			</nav>
		</div>

		<div class="hidden sm:flex items-center gap-1">
		<!-- Settings Link (Desktop) -->
		<a
			href="/settings"
			class="flex items-center justify-center w-9 h-9 rounded-lg text-base-content/50 hover:text-base-content hover:bg-base-200 transition-colors"
			aria-label="Settings"
		>
			<svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
				<path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
				<path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
			</svg>
		</a>
		</div>

		<!-- Mobile Menu Button -->
		<button
			type="button"
			class="sm:hidden flex items-center justify-center w-10 h-10 rounded-lg text-base-content/60 hover:text-base-content hover:bg-base-200 transition-colors"
			aria-label={mobileMenuOpen ? 'Close menu' : 'Open menu'}
			aria-expanded={mobileMenuOpen}
			onclick={toggleMobileMenu}
		>
			{#if mobileMenuOpen}
				<svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
					<path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
				</svg>
			{:else}
				<svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
					<path stroke-linecap="round" stroke-linejoin="round" d="M4 6h16M4 12h16M4 18h16" />
				</svg>
			{/if}
		</button>
	</div>

	<!-- Mobile Menu -->
	{#if mobileMenuOpen}
		<div class="sm:hidden border-t border-base-300 mt-3 pt-4 animate-fade-in">
			<nav class="space-y-1 mb-4">
				{#each items as item}
					<a
						href={item.href}
						class="nav-link"
						class:active={isActive(item.href)}
						onclick={closeMobileMenu}
					>
						{#if item.icon && icons[item.icon]}
							<svg
								class="w-[18px] h-[18px]"
								viewBox={icons[item.icon].viewBox}
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
							>
								{@html icons[item.icon].path}
							</svg>
						{/if}
						{item.label}
					</a>
				{/each}
			</nav>
			<div class="border-t border-base-300 pt-4 space-y-1">
				<a
					href="/settings"
					class="nav-link"
					class:active={currentPath.startsWith('/settings')}
					onclick={closeMobileMenu}
				>
					<svg class="w-[18px] h-[18px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
						<path stroke-linecap="round" stroke-linejoin="round" d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z" />
						<path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
					</svg>
					Settings
				</a>
			</div>
		</div>
	{/if}
</header>
