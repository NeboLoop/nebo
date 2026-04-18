<!--
  AppNav Component — V2 Header
  4-column grid: Brand | Nav Tabs | Search + Redeem | Utilities + Avatar
-->

<script lang="ts">
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import { NeboIcon } from '$lib/components/icons';
	import { updateInfo } from '$lib/stores/update';
	import {
		Search,
		Link2,
		Settings,
		Menu,
		X,
		Zap,
		Home,
		Bot,
		Store,
		LayoutDashboard
	} from 'lucide-svelte';

	interface Props {
		userName?: string;
	}

	let { userName = '' }: Props = $props();

	const currentPath = $derived($page.url.pathname);
	const hasUpdate = $derived($updateInfo?.available === true);
	const userInitial = $derived(userName ? userName.charAt(0).toUpperCase() : '?');

	let mobileMenuOpen = $state(false);

	function toggleMobileMenu() {
		mobileMenuOpen = !mobileMenuOpen;
	}

	function closeMobileMenu() {
		mobileMenuOpen = false;
	}

	function isActive(href: string): boolean {
		if (href === '/') {
			return currentPath === '/';
		}
		// Both /agents (management) and /agent/* (individual) highlight the Agents tab
		if (href === '/agents') {
			return currentPath.startsWith('/agents') || currentPath.startsWith('/agent');
		}
		return currentPath.startsWith(href);
	}

	function openCommandPalette() {
		window.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'k', metaKey: true, bubbles: true })
		);
	}

	function openRedeem() {
		window.dispatchEvent(new CustomEvent('nebo:open-redeem'));
	}

	// Nav tabs for the v2 header
	const navTabs = [
		{ label: 'Home', href: '/', icon: Home },
		{ label: 'Agents', href: '/agents', icon: Bot },
		{ label: 'Marketplace', href: '/marketplace', icon: Store }
	];
</script>

<!-- Desktop Header (v2 design) -->
<header class="v2-header hidden md:grid">
	<!-- 1. Brand -->
	<a href="/" class="v2-header-brand no-underline" title={$t('nav.home')} aria-label={$t('nav.home')}>
		<div class="v2-header-brand-mark">
			<NeboIcon class="w-[18px] h-[18px]" />
		</div>
		<div>
			<div class="v2-header-brand-name">Nebo</div>
			<div class="v2-header-brand-beta">{$t('common.beta')}</div>
		</div>
	</a>

	<!-- 2. Nav Tabs -->
	<nav class="v2-nav-tabs" aria-label={$t('nav.pageNavigation')}>
		{#each navTabs as tab}
			<a
				href={tab.href}
				class="v2-nav-tab {isActive(tab.href) ? 'v2-nav-tab-active' : ''}"
			>
				{tab.label}
			</a>
		{/each}
	</nav>

	<!-- 3. Center-Right: Search + Redeem -->
	<div class="v2-header-center-right">
		<button type="button" class="v2-search-pill" onclick={openCommandPalette} aria-label={$t('nav.search')}>
			<Search class="w-[14px] h-[14px]" />
			<span>Search agents, chats, workspaces…</span>
			<span class="v2-kbd">⌘K</span>
		</button>
		<button type="button" class="v2-redeem-btn" onclick={openRedeem} title="Redeem install code">
			<Zap class="w-3 h-3" />
			Redeem code
		</button>
	</div>

	<!-- 4. Right: Icons + Avatar -->
	<div class="v2-header-right">
		<a href="/integrations" class="v2-header-icon-btn" title={$t('nav.connectors')} aria-label={$t('nav.connectors')}>
			<Link2 class="w-[17px] h-[17px]" />
		</a>
		<a
			href={hasUpdate ? "/settings/about" : "/settings/account"}
			class="v2-header-icon-btn relative"
			title={hasUpdate ? $t('nav.updateAvailable') : $t('nav.settings')}
			aria-label={$t('nav.settings')}
		>
			<Settings class="w-[17px] h-[17px]" />
			{#if hasUpdate}
				<span class="v2-update-dot"></span>
			{/if}
		</a>
		<div class="v2-header-sep"></div>
		<a href="/settings/account" class="v2-user-avatar" title={userName || 'Account'}>
			{userInitial}
		</a>
	</div>
</header>

<!-- Mobile Header (compact) -->
<header class="layout-app-header md:hidden">
	<div class="w-full mx-auto flex items-center justify-between gap-4">
		<a href="/" class="flex items-center gap-2 no-underline" title={$t('nav.home')} aria-label={$t('nav.home')}>
			<NeboIcon class="w-10 h-10" />
			<div class="flex flex-col leading-none">
				<span class="font-display text-xl font-bold text-base-content tracking-tight">Nebo</span>
				<span class="text-xs font-semibold uppercase tracking-widest text-base-content/80">{$t('common.beta')}</span>
			</div>
		</a>
		<button
			type="button"
			class="flex items-center justify-center w-10 h-10 rounded-lg text-base-content/90 hover:text-base-content hover:bg-base-200 transition-colors"
			aria-label={$t('nav.openMenu')}
			title={$t('nav.openMenu')}
			onclick={toggleMobileMenu}
		>
			<Menu class="w-6 h-6" />
		</button>
	</div>
</header>

<!-- Mobile Drawer -->
{#if mobileMenuOpen}
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div
		class="mobile-drawer-backdrop md:hidden"
		onclick={closeMobileMenu}
		onkeydown={(e) => e.key === 'Escape' && closeMobileMenu()}
	>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<nav class="mobile-drawer" onclick={(e) => e.stopPropagation()}>
			<div class="flex items-center justify-between px-4 py-3 border-b border-base-300">
				<a href="/" class="flex items-center gap-2 no-underline" onclick={closeMobileMenu}>
					<NeboIcon class="w-8 h-8" />
					<span class="font-display text-lg font-bold text-base-content tracking-tight">Nebo</span>
				</a>
				<button
					type="button"
					class="flex items-center justify-center w-8 h-8 rounded-lg text-base-content/90 hover:text-base-content hover:bg-base-200 transition-colors"
					aria-label={$t('nav.closeMenu')}
					title={$t('nav.closeMenu')}
					onclick={closeMobileMenu}
				>
					<X class="w-5 h-5" />
				</button>
			</div>

			<div class="flex flex-col gap-1 p-3">
				<a href="/" class="nav-link" class:active={isActive('/')} onclick={closeMobileMenu}>
					<LayoutDashboard class="w-[18px] h-[18px]" />
					{$t('nav.chat')}
				</a>
				<a href="/agents" class="nav-link" class:active={isActive('/agents')} onclick={closeMobileMenu}>
					<Bot class="w-[18px] h-[18px]" />
					Agents
				</a>
			</div>

			<div class="border-t border-base-300 p-3 flex flex-col gap-1">
				<a
					href="/marketplace"
					class="nav-link"
					class:active={currentPath.startsWith('/marketplace')}
					onclick={closeMobileMenu}
				>
					<Store class="w-[18px] h-[18px]" />
					{$t('nav.marketplace')}
				</a>
				<a
					href="/integrations"
					class="nav-link"
					class:active={currentPath.startsWith('/integrations')}
					onclick={closeMobileMenu}
				>
					<Link2 class="w-[18px] h-[18px]" />
					{$t('nav.connectors')}
				</a>
				<a
					href="/settings/account"
					class="nav-link"
					class:active={currentPath.startsWith('/settings')}
					onclick={closeMobileMenu}
				>
					<Settings class="w-[18px] h-[18px]" />
					{$t('nav.settings')}
				</a>
			</div>
		</nav>
	</div>
{/if}
