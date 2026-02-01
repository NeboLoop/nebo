<!--
  AppNav Component
  Navigation for the authenticated app dashboard
-->

<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { Avatar, DropdownMenu } from '$lib/components/ui';
	import { auth, currentUser } from '$lib/stores';

	interface NavItem {
		label: string;
		href: string;
		icon?: 'dashboard' | 'history' | 'settings' | 'analytics' | 'agent' | 'tools' | 'channels' | 'mcp' | 'status' | 'memories' | 'tasks';
	}

	let {
		items = [
			{ label: 'Agent', href: '/agent', icon: 'agent' },
			{ label: 'Memories', href: '/memories', icon: 'memories' },
			{ label: 'Tasks', href: '/tasks', icon: 'tasks' },
			{ label: 'Sessions', href: '/sessions', icon: 'history' },
			{ label: 'Extensions', href: '/tools', icon: 'tools' },
			{ label: 'Channels', href: '/channels', icon: 'channels' },
			{ label: 'MCP', href: '/mcp', icon: 'mcp' },
			{ label: 'Status', href: '/status', icon: 'status' }
		] as NavItem[]
	}: {
		items?: NavItem[];
	} = $props();

	const currentPath = $derived($page.url.pathname);

	const userInitials = $derived(
		$currentUser?.name
			? $currentUser.name
					.split(' ')
					.map((n) => n[0])
					.join('')
					.toUpperCase()
					.slice(0, 2)
			: 'U'
	);

	let mobileMenuOpen = $state(false);

	function toggleMobileMenu() {
		mobileMenuOpen = !mobileMenuOpen;
	}

	function closeMobileMenu() {
		mobileMenuOpen = false;
	}

	function handleLogout() {
		auth.logout();
		goto('/auth/login');
	}

	const userMenuItems = [
		{ label: 'Settings', onClick: () => goto('/settings') },
		{ separator: true, label: '' },
		{ label: 'Sign Out', onClick: handleLogout }
	];

	const icons: Record<string, { viewBox: string; path: string }> = {
		dashboard: {
			viewBox: '0 0 24 24',
			path: '<rect x="3" y="3" width="7" height="7"/><rect x="14" y="3" width="7" height="7"/><rect x="14" y="14" width="7" height="7"/><rect x="3" y="14" width="7" height="7"/>'
		},
		history: {
			viewBox: '0 0 24 24',
			path: '<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>'
		},
		settings: {
			viewBox: '0 0 24 24',
			path: '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>'
		},
		analytics: {
			viewBox: '0 0 24 24',
			path: '<line x1="18" y1="20" x2="18" y2="10"/><line x1="12" y1="20" x2="12" y2="4"/><line x1="6" y1="20" x2="6" y2="14"/>'
		},
		agent: {
			viewBox: '0 0 24 24',
			path: '<path d="M12 8V4H8"/><rect x="8" y="8" width="8" height="8" rx="1"/><path d="M12 16v4h4"/><path d="M8 12H4"/><path d="M20 12h-4"/>'
		},
		tools: {
			viewBox: '0 0 24 24',
			path: '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>'
		},
		channels: {
			viewBox: '0 0 24 24',
			path: '<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/>'
		},
		mcp: {
			viewBox: '0 0 24 24',
			path: '<rect x="2" y="2" width="20" height="8" rx="2" ry="2"/><rect x="2" y="14" width="20" height="8" rx="2" ry="2"/><line x1="6" y1="6" x2="6.01" y2="6"/><line x1="6" y1="18" x2="6.01" y2="18"/>'
		},
		status: {
			viewBox: '0 0 24 24',
			path: '<polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/>'
		},
		memories: {
			viewBox: '0 0 24 24',
			path: '<path d="M12 2a9 9 0 0 0-9 9c0 4.17 3.08 7.68 7 8.55V21a1 1 0 0 0 2 0v-1.45c3.92-.87 7-4.38 7-8.55a9 9 0 0 0-9-9z"/><path d="M9 10a1 1 0 1 1-2 0 1 1 0 0 1 2 0z"/><path d="M17 10a1 1 0 1 1-2 0 1 1 0 0 1 2 0z"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/>'
		},
		tasks: {
			viewBox: '0 0 24 24',
			path: '<circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/>'
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
	<div class="w-full max-w-[1400px] mx-auto flex items-center justify-between gap-4">
		<div class="flex items-center gap-8">
			<!-- Logo -->
			<a href="/" class="flex items-center no-underline">
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

		<!-- User Menu (Desktop) -->
		<div class="hidden sm:block">
			<DropdownMenu items={userMenuItems}>
				{#snippet trigger()}
					<div
						class="flex items-center gap-2.5 px-2 py-1.5 rounded-lg hover:bg-base-200 transition-colors cursor-pointer"
					>
						<Avatar initials={userInitials} size="sm" />
						<span class="text-sm font-medium text-base-content/70">
							{$currentUser?.name ?? 'Account'}
						</span>
						<svg
							class="w-4 h-4 text-base-content/50"
							fill="none"
							viewBox="0 0 24 24"
							stroke="currentColor"
							stroke-width="2"
						>
							<path stroke-linecap="round" stroke-linejoin="round" d="M19 9l-7 7-7-7" />
						</svg>
					</div>
				{/snippet}
			</DropdownMenu>
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
					<Avatar initials={userInitials} size="xs" />
					Settings
				</a>
				<button
					type="button"
					class="nav-link w-full text-left text-error"
					onclick={() => {
						closeMobileMenu();
						handleLogout();
					}}
				>
					<svg
						class="w-[18px] h-[18px]"
						fill="none"
						viewBox="0 0 24 24"
						stroke="currentColor"
						stroke-width="2"
					>
						<path
							stroke-linecap="round"
							stroke-linejoin="round"
							d="M17 16l4-4m0 0l-4-4m4 4H7m6 4v1a3 3 0 01-3 3H6a3 3 0 01-3-3V7a3 3 0 013-3h4a3 3 0 013 3v1"
						/>
					</svg>
					Sign Out
				</button>
			</div>
		</div>
	{/if}
</header>
