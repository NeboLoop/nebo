<!--
  AppNav Component — Minimal Header
  2-column grid: Brand | Search
-->

<script lang="ts">
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import { NeboIcon } from '$lib/components/icons';
	import {
		Search,
		Menu,
		X,
		Store,
		Settings
	} from 'lucide-svelte';

	interface Props {
		userName?: string;
	}

	let { userName = '' }: Props = $props();

	const currentPath = $derived($page.url.pathname);

	let mobileMenuOpen = $state(false);

	function toggleMobileMenu() {
		mobileMenuOpen = !mobileMenuOpen;
	}

	function closeMobileMenu() {
		mobileMenuOpen = false;
	}

	function openCommandPalette() {
		window.dispatchEvent(
			new KeyboardEvent('keydown', { key: 'k', metaKey: true, bubbles: true })
		);
	}
</script>

<!-- Desktop Header (minimal: brand + search) -->
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

	<!-- 2. Search -->
	<div class="flex justify-end items-center">
		<button type="button" class="v2-search-pill" onclick={openCommandPalette} aria-label={$t('nav.search')}>
			<Search class="w-[14px] h-[14px]" />
			<span>Search agents, chats, workspaces…</span>
			<span class="v2-kbd">⌘K</span>
		</button>
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
