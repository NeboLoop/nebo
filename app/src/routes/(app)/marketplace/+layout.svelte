<script lang="ts">
	import { page } from '$app/stores';
	import { t } from 'svelte-i18n';
	import type { Snippet } from 'svelte';
	import {
		Compass,
		UserCog,
		FileText,
		Grid3x3,
		PackageCheck
	} from 'lucide-svelte';

	let { children }: { children: Snippet } = $props();

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

		<!-- Publish: hidden until SDK is ready -->
		<!-- <div class="marketplace-sidebar-footer">
			<a href="/marketplace" class="marketplace-nav-link">
				<Code2 class="w-4.5 h-4.5" strokeWidth={1.5} />
				<span>Publish</span>
			</a>
		</div> -->
	</aside>

	<main class="marketplace-content">
		{@render children()}
	</main>
</div>
