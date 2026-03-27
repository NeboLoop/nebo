<!--
  SideNav Component
  Collapsible vertical rail for page navigation (Dashboard, Agents, etc.)
  Collapses to icon-only rail, expands to show labels.
  Desktop only — mobile uses the AppNav drawer.
-->

<script lang="ts">
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import {
		LayoutDashboard,
		PanelLeftClose,
		PanelLeft
	} from 'lucide-svelte';

	interface NavItem {
		label: string;
		href: string;
		icon: typeof LayoutDashboard;
	}

	const navItems: NavItem[] = [
		{ label: 'nav.chat', href: '/', icon: LayoutDashboard }
	];

	const currentPath = $derived($page.url.pathname);

	let collapsed = $state(false);

	if (typeof window !== 'undefined') {
		collapsed = localStorage.getItem('nebo:sidebar') === 'collapsed';
	}

	function toggleCollapse() {
		collapsed = !collapsed;
		localStorage.setItem('nebo:sidebar', collapsed ? 'collapsed' : 'expanded');
	}

	function isActive(href: string): boolean {
		if (href === '/') {
			return currentPath === '/';
		}
		return currentPath.startsWith(href);
	}
</script>

<aside
	class="hidden"
>
	<!-- Nav Items -->
	<nav
		class="flex flex-col gap-0.5 flex-1 py-3 {collapsed ? 'items-center w-full px-1' : ''}"
		aria-label={$t('nav.pageNavigation')}
	>
		{#each navItems as item}
			<a
				href={item.href}
				class="nav-link {collapsed ? 'justify-center px-0 w-10 h-10' : ''}"
				class:active={isActive(item.href)}
				title={collapsed ? $t(item.label) : undefined}
			>
				<item.icon class="w-[18px] h-[18px] shrink-0" />
				{#if !collapsed}
					{$t(item.label)}
				{/if}
			</a>
		{/each}
	</nav>

	<!-- Collapse Toggle -->
	<div class="pb-3 {collapsed ? 'flex justify-center w-full px-1' : ''}">
		<button
			type="button"
			class="nav-link {collapsed ? 'justify-center px-0 w-10 h-10' : ''}"
			title={collapsed ? $t('nav.expandSidebar') : $t('nav.collapseSidebar')}
			onclick={toggleCollapse}
		>
			{#if collapsed}
				<PanelLeft class="w-[18px] h-[18px] shrink-0" />
			{:else}
				<PanelLeftClose class="w-[18px] h-[18px] shrink-0" />
				<span class="flex-1">{$t('common.collapse')}</span>
			{/if}
		</button>
	</div>
</aside>
