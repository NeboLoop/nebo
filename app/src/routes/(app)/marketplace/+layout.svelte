<script lang="ts">
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import type { Snippet } from 'svelte';
	import {
		Compass,
		UserCog,
		FileText,
		Grid3x3,
		PackageCheck,
		ArrowLeft
	} from 'lucide-svelte';
	import AvatarMenu from '$lib/components/sidebar/AvatarMenu.svelte';
	import * as api from '$lib/api/nebo';
	import { onMount } from 'svelte';

	let { children }: { children: Snippet } = $props();
	let userName = $state('');

	onMount(async () => {
		try {
			const res = await api.getUserProfile();
			userName = res.profile?.displayName || '';
		} catch { /* fine */ }
	});

	const currentPath = $derived($page.url.pathname);

	const pageTitle = $derived.by(() => {
		if (currentPath.startsWith('/marketplace/skills')) return $t('marketplace.skills');
		if (currentPath.startsWith('/marketplace/installed')) return $t('marketplace.installed');
		if (currentPath.startsWith('/marketplace/categories')) return $t('marketplace.categories');
		if (currentPath.startsWith('/marketplace/agents')) return $t('marketplace.agents');
		return $t('marketplace.title');
	});

	function isActive(href: string): boolean {
		if (href === '/marketplace') return currentPath === '/marketplace';
		return currentPath === href || currentPath.startsWith(href + '/');
	}

	const navItems = $derived([
		{ label: $t('marketplace.featured'), icon: Compass, href: '/marketplace' },
		{ label: $t('marketplace.agents'), icon: UserCog, href: '/marketplace/agents' },
		{ label: $t('marketplace.skills'), icon: FileText, href: '/marketplace/skills' },
		{ label: $t('marketplace.installed'), icon: PackageCheck, href: '/marketplace/installed' },
	]);

	const utilItems = $derived([
		{ label: $t('marketplace.categories'), icon: Grid3x3, href: '/marketplace/categories' },
	]);
</script>

<svelte:head>
	<title>{pageTitle} - Nebo</title>
</svelte:head>

<div class="marketplace-layout">
	<aside class="marketplace-sidebar">
		<div class="marketplace-sidebar-header">
			<a href="/" class="marketplace-nav-link text-base-content/60 no-underline">
				<ArrowLeft class="w-4 h-4" />
				<span class="text-[13px]">Back to chats</span>
			</a>
			<h2 class="marketplace-sidebar-title">{$t('marketplace.title')}</h2>
		</div>

		<nav class="marketplace-sidebar-nav">
			{#each navItems as item}
				{@const Icon = item.icon}
				<a
					href={item.href}
					class="marketplace-nav-link"
					class:active={isActive(item.href)}
				>
					<Icon class="w-4.5 h-4.5" strokeWidth={1.5} />
					<span>{item.label}</span>
				</a>
			{/each}

			<div class="marketplace-nav-divider"></div>

			{#each utilItems as item}
				{@const Icon = item.icon}
				<a
					href={item.href}
					class="marketplace-nav-link"
					class:active={isActive(item.href)}
				>
					<Icon class="w-4.5 h-4.5" strokeWidth={1.5} />
					<span>{item.label}</span>
				</a>
			{/each}
		</nav>

		<div class="mt-auto px-2.5 pb-2.5 pt-1 border-t border-base-300">
			<AvatarMenu {userName} />
		</div>
	</aside>

	<main class="marketplace-content">
		{@render children()}
	</main>
</div>
