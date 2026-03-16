<script lang="ts">
	import { page } from '$app/stores';
	import type { Snippet } from 'svelte';
	import {
		Compass,
		UserCog,
		FileText,
		GitBranch,
		Grid3x3,
		Code2
	} from 'lucide-svelte';

	let { children }: { children: Snippet } = $props();

	const currentPath = $derived($page.url.pathname);

	const pageTitle = $derived.by(() => {
		if (currentPath.startsWith('/marketplace/roles')) return 'Roles';
		if (currentPath.startsWith('/marketplace/workflows')) return 'Workflows';
		if (currentPath.startsWith('/marketplace/skills')) return 'Skills';
		if (currentPath.startsWith('/marketplace/categories')) return 'Categories';
		return 'Marketplace';
	});

	function isActive(href: string): boolean {
		if (href === '/marketplace') return currentPath === '/marketplace';
		return currentPath === href || currentPath.startsWith(href + '/');
	}

	const navItems = [
		{ label: 'Featured', icon: Compass, href: '/marketplace' },
		{ label: 'Roles', icon: UserCog, href: '/marketplace/roles' },
		{ label: 'Skills', icon: FileText, href: '/marketplace/skills' },
		{ label: 'Workflows', icon: GitBranch, href: '/marketplace/workflows' },
	];

	const utilItems = [
		{ label: 'Categories', icon: Grid3x3, href: '/marketplace/categories' },
	];
</script>

<svelte:head>
	<title>{pageTitle} - Nebo</title>
</svelte:head>

<div class="marketplace-layout">
	<aside class="marketplace-sidebar">
		<div class="marketplace-sidebar-header">
			<h2 class="marketplace-sidebar-title">Marketplace</h2>
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

		<div class="marketplace-sidebar-footer">
			<a href="/marketplace" class="marketplace-nav-link">
				<Code2 class="w-4.5 h-4.5" strokeWidth={1.5} />
				<span>Publish</span>
			</a>
		</div>
	</aside>

	<main class="marketplace-content">
		{@render children()}
	</main>
</div>
