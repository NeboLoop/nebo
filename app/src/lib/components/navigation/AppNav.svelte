<!--
  AppNav Component
  Top header bar: logo, search, marketplace, settings
  Page nav items (Dashboard, Agents, etc.) live in the SideNav component
-->

<script lang="ts">
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import { NeboIcon } from '$lib/components/icons';
	import { updateInfo } from '$lib/stores/update';
	import {
		Search,
		Puzzle,
		Link2,
		Settings,
		Menu,
		X,
		LayoutDashboard
	} from 'lucide-svelte';

	const currentPath = $derived($page.url.pathname);
	const hasUpdate = $derived($updateInfo?.available === true);

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
		return currentPath.startsWith(href);
	}

	// Mobile-only nav items (these are in the SideNav on desktop)
	const mobileNavItems = [
		{ labelKey: 'nav.chat', href: '/', icon: LayoutDashboard }
	];
</script>

<header class="layout-app-header">
	<div class="w-full mx-auto flex items-center justify-between gap-4">
		<div class="flex items-center gap-8">
			<!-- Logo -->
			<a href="/" class="flex items-center gap-2 no-underline" title={$t('nav.home')} aria-label={$t('nav.home')}>
				<NeboIcon class="w-10 h-10" />
				<div class="flex flex-col leading-none">
					<span class="font-display text-xl font-bold text-base-content tracking-tight">Nebo</span>
					<span class="text-xs font-semibold uppercase tracking-widest text-base-content/80"
						>{$t('common.beta')}</span
					>
				</div>
			</a>
		</div>

		<div class="hidden md:flex items-center gap-1">
			<!-- Command Palette Trigger -->
			<button
				type="button"
				class="flex items-center gap-1.5 h-9 px-2.5 rounded-lg transition-colors text-base-content/60 hover:text-base-content hover:bg-base-200"
				aria-label={$t('nav.search')}
				title={$t('nav.search')}
				onclick={() => {
					window.dispatchEvent(
						new KeyboardEvent('keydown', { key: 'k', metaKey: true, bubbles: true })
					);
				}}
			>
				<Search class="w-4 h-4" />
				<kbd class="kbd kbd-sm text-sm opacity-60">&#8984;K</kbd>
			</button>
			<!-- Marketplace Link -->
			<a
				href="/marketplace"
				title={$t('nav.marketplace')}
				class="flex items-center justify-center w-9 h-9 rounded-lg transition-colors {isActive(
					'/marketplace'
				)
					? 'text-primary bg-primary/10'
					: 'text-base-content/90 hover:text-base-content hover:bg-base-200'}"
				aria-label={$t('nav.marketplace')}
			>
				<Puzzle class="w-5 h-5" />
			</a>
			<!-- Connectors Link -->
			<a
				href="/integrations"
				title={$t('nav.connectors')}
				class="flex items-center justify-center w-9 h-9 rounded-lg transition-colors {isActive(
					'/integrations'
				)
					? 'text-primary bg-primary/10'
					: 'text-base-content/90 hover:text-base-content hover:bg-base-200'}"
				aria-label={$t('nav.connectors')}
			>
				<Link2 class="w-5 h-5" />
			</a>
			<!-- Settings Link -->
			<a
				href={hasUpdate ? "/settings/about" : "/settings/account"}
				title={hasUpdate ? $t('nav.updateAvailable') : $t('nav.settings')}
				class="relative flex items-center justify-center w-9 h-9 rounded-lg transition-colors {isActive(
					'/settings'
				)
					? 'text-primary bg-primary/10'
					: 'text-base-content/90 hover:text-base-content hover:bg-base-200'}"
				aria-label={$t('nav.settings')}
			>
				<Settings class="w-5 h-5" />
				{#if hasUpdate}
					<span class="absolute top-1 right-1 w-2.5 h-2.5 rounded-full bg-info border-2 border-base-100"></span>
				{/if}
			</a>
		</div>

		<!-- Mobile Menu Button -->
		<button
			type="button"
			class="md:hidden flex items-center justify-center w-10 h-10 rounded-lg text-base-content/90 hover:text-base-content hover:bg-base-200 transition-colors"
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
				{#each mobileNavItems as item}
					<a
						href={item.href}
						class="nav-link"
						class:active={isActive(item.href)}
						onclick={closeMobileMenu}
					>
						<item.icon class="w-[18px] h-[18px]" />
						{$t(item.labelKey)}
					</a>
				{/each}
			</div>

			<div class="border-t border-base-300 p-3 flex flex-col gap-1">
				<a
					href="/marketplace"
					class="nav-link"
					class:active={currentPath.startsWith('/marketplace')}
					onclick={closeMobileMenu}
				>
					<Puzzle class="w-[18px] h-[18px]" />
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
