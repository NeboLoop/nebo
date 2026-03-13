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
			{ label: 'Agents', href: '/agent', icon: 'agent' },
			{ label: 'Roles', href: '/roles', icon: 'roles' },
			{ label: 'Workflows', href: '/workflows', icon: 'workflows' },
			{ label: 'Skills', href: '/skills', icon: 'skills' },
			{ label: 'Integrations', href: '/integrations', icon: 'integrations' },
			{ label: 'Events', href: '/events', icon: 'events' }
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
		},
		roles: {
			viewBox: '0 0 24 24',
			path: '<circle cx="12" cy="8" r="5"/><path d="M20 21a8 8 0 0 0-16 0"/>'
		},
		workflows: {
			viewBox: '0 0 24 24',
			path: '<circle cx="18" cy="18" r="3"/><circle cx="6" cy="6" r="3"/><path d="M6 21V9a9 9 0 0 0 9 9"/>'
		},
		skills: {
			viewBox: '0 0 24 24',
			path: '<path d="M13 2 3 14h9l-1 8 10-12h-9l1-8z"/>'
		},
		integrations: {
			viewBox: '0 0 24 24',
			path: '<path d="M12 2v4"/><path d="M12 18v4"/><path d="m4.93 4.93 2.83 2.83"/><path d="m16.24 16.24 2.83 2.83"/><path d="M2 12h4"/><path d="M18 12h4"/><path d="m4.93 19.07 2.83-2.83"/><path d="m16.24 7.76 2.83-2.83"/>'
		},
		events: {
			viewBox: '0 0 24 24',
			path: '<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/>'
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
				<div class="flex flex-col leading-none">
					<span class="font-display text-xl font-bold text-base-content tracking-tight">Nebo</span>
					<span class="text-[10px] font-semibold uppercase tracking-widest text-base-content/70"
						>alpha</span
					>
				</div>
			</a>

			<!-- Desktop Navigation -->
			<nav class="hidden md:flex items-center gap-1" aria-label="Main navigation">
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

		<div class="hidden md:flex items-center gap-1">
			<!-- Store Link (Desktop) -->
			<a
				href="/store"
				class="flex items-center justify-center w-9 h-9 rounded-lg transition-colors {isActive('/store') ? 'text-primary bg-primary/10' : 'text-base-content/70 hover:text-base-content hover:bg-base-200'}"
				aria-label="Store"
			>
				<svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
					<path stroke-linecap="round" stroke-linejoin="round" d="M14.25 6.087c0-.355.186-.676.401-.959.221-.29.349-.634.349-1.003 0-1.036-1.007-1.875-2.25-1.875s-2.25.84-2.25 1.875c0 .369.128.713.349 1.003.215.283.401.604.401.959v0a.64.64 0 01-.657.643 48.39 48.39 0 01-4.163-.3c.186 1.613.293 3.25.315 4.907a.656.656 0 01-.658.663v0c-.355 0-.676-.186-.959-.401a1.647 1.647 0 00-1.003-.349c-1.036 0-1.875 1.007-1.875 2.25s.84 2.25 1.875 2.25c.369 0 .713-.128 1.003-.349.283-.215.604-.401.959-.401v0c.31 0 .555.26.532.57a48.039 48.039 0 01-.642 5.056c1.518.19 3.058.309 4.616.354a.64.64 0 00.657-.643v0c0-.355-.186-.676-.401-.959a1.647 1.647 0 01-.349-1.003c0-1.035 1.008-1.875 2.25-1.875 1.243 0 2.25.84 2.25 1.875 0 .369-.128.713-.349 1.003-.215.283-.4.604-.4.959v0c0 .333.277.599.61.58a48.1 48.1 0 005.427-.63 48.05 48.05 0 00.582-4.717.532.532 0 00-.533-.57v0c-.355 0-.676.186-.959.401-.29.221-.634.349-1.003.349-1.035 0-1.875-1.007-1.875-2.25s.84-2.25 1.875-2.25c.37 0 .713.128 1.003.349.283.215.604.401.96.401v0a.656.656 0 00.658-.663 48.422 48.422 0 00-.37-5.36c-1.886.342-3.81.574-5.766.689a.578.578 0 01-.61-.58v0z" />
				</svg>
			</a>
			<!-- Settings Link (Desktop) -->
			<a
				href="/settings/account"
				class="flex items-center justify-center w-9 h-9 rounded-lg transition-colors {isActive('/settings') ? 'text-primary bg-primary/10' : 'text-base-content/70 hover:text-base-content hover:bg-base-200'}"
				aria-label="Settings"
			>
				<svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						d="M10.325 4.317c.426-1.756 2.924-1.756 3.35 0a1.724 1.724 0 002.573 1.066c1.543-.94 3.31.826 2.37 2.37a1.724 1.724 0 001.066 2.573c1.756.426 1.756 2.924 0 3.35a1.724 1.724 0 00-1.066 2.573c.94 1.543-.826 3.31-2.37 2.37a1.724 1.724 0 00-2.573 1.066c-.426 1.756-2.924 1.756-3.35 0a1.724 1.724 0 00-2.573-1.066c-1.543.94-3.31-.826-2.37-2.37a1.724 1.724 0 00-1.066-2.573c-1.756-.426-1.756-2.924 0-3.35a1.724 1.724 0 001.066-2.573c-.94-1.543.826-3.31 2.37-2.37.996.608 2.296.07 2.572-1.065z"
					/>
					<path
						stroke-linecap="round"
						stroke-linejoin="round"
						d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
					/>
				</svg>
			</a>
		</div>

		<!-- Mobile Menu Button -->
		<button
			type="button"
			class="md:hidden flex items-center justify-center w-10 h-10 rounded-lg text-base-content/70 hover:text-base-content hover:bg-base-200 transition-colors"
			aria-label="Open menu"
			onclick={toggleMobileMenu}
		>
			<svg class="w-6 h-6" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
				<path stroke-linecap="round" stroke-linejoin="round" d="M4 6h16M4 12h16M4 18h16" />
			</svg>
		</button>
	</div>

</header>

<!-- Mobile Drawer -->
{#if mobileMenuOpen}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="mobile-drawer-backdrop md:hidden" onclick={closeMobileMenu} onkeydown={(e) => e.key === 'Escape' && closeMobileMenu()}>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<nav class="mobile-drawer" onclick={(e) => e.stopPropagation()}>
			<div class="flex items-center justify-between px-4 py-3 border-b border-base-300">
				<a href="/" class="flex items-center gap-2 no-underline" onclick={closeMobileMenu}>
					<NeboIcon class="w-8 h-8" />
					<span class="font-display text-lg font-bold text-base-content tracking-tight">Nebo</span>
				</a>
				<button
					type="button"
					class="flex items-center justify-center w-8 h-8 rounded-lg text-base-content/70 hover:text-base-content hover:bg-base-200 transition-colors"
					aria-label="Close menu"
					onclick={closeMobileMenu}
				>
					<svg class="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
						<path stroke-linecap="round" stroke-linejoin="round" d="M6 18L18 6M6 6l12 12" />
					</svg>
				</button>
			</div>

			<div class="flex flex-col gap-1 p-3">
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
			</div>

			<div class="border-t border-base-300 p-3 flex flex-col gap-1">
				<a
					href="/store"
					class="nav-link"
					class:active={currentPath.startsWith('/store')}
					onclick={closeMobileMenu}
				>
					<svg class="w-[18px] h-[18px]" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
						<path stroke-linecap="round" stroke-linejoin="round" d="M14.25 6.087c0-.355.186-.676.401-.959.221-.29.349-.634.349-1.003 0-1.036-1.007-1.875-2.25-1.875s-2.25.84-2.25 1.875c0 .369.128.713.349 1.003.215.283.401.604.401.959v0a.64.64 0 01-.657.643 48.39 48.39 0 01-4.163-.3c.186 1.613.293 3.25.315 4.907a.656.656 0 01-.658.663v0c-.355 0-.676-.186-.959-.401a1.647 1.647 0 00-1.003-.349c-1.036 0-1.875 1.007-1.875 2.25s.84 2.25 1.875 2.25c.369 0 .713-.128 1.003-.349.283-.215.604-.401.959-.401v0c.31 0 .555.26.532.57a48.039 48.039 0 01-.642 5.056c1.518.19 3.058.309 4.616.354a.64.64 0 00.657-.643v0c0-.355-.186-.676-.401-.959a1.647 1.647 0 01-.349-1.003c0-1.035 1.008-1.875 2.25-1.875 1.243 0 2.25.84 2.25 1.875 0 .369-.128.713-.349 1.003-.215.283-.4.604-.4.959v0c0 .333.277.599.61.58a48.1 48.1 0 005.427-.63 48.05 48.05 0 00.582-4.717.532.532 0 00-.533-.57v0c-.355 0-.676.186-.959.401-.29.221-.634.349-1.003.349-1.035 0-1.875-1.007-1.875-2.25s.84-2.25 1.875-2.25c.37 0 .713.128 1.003.349.283.215.604.401.96.401v0a.656.656 0 00.658-.663 48.422 48.422 0 00-.37-5.36c-1.886.342-3.81.574-5.766.689a.578.578 0 01-.61-.58v0z" />
					</svg>
					Store
				</a>
				<a
					href="/settings/account"
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
		</nav>
	</div>
{/if}
