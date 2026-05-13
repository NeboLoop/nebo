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

<div class="flex min-h-0 h-full">
	<aside class="border-r border-base-300 bg-base-200 flex flex-col h-full min-h-0 overflow-hidden w-[260px] min-w-[260px]">
		<div class="px-2.5 pt-2.5 pb-1.5 flex flex-col gap-0.5">
			<!-- Back to chats -->
			<a
				href="/"
				class="flex items-center gap-2.5 px-2.5 py-2 rounded-lg text-[13.5px] text-base-content/60 no-underline hover:bg-base-300 hover:text-base-content transition-colors"
			>
				<ArrowLeft class="w-[15px] h-[15px]" />
				Back to chats
			</a>
			<div class="text-[10.5px] font-semibold tracking-[0.8px] text-base-content/40 uppercase px-2.5 pt-3.5 pb-1">
				{$t('marketplace.title')}
			</div>
		</div>

		<div class="flex-1 overflow-auto px-2.5 pb-4">
			{#each navItems as item}
				{@const Icon = item.icon}
				<a
					href={item.href}
					class="flex items-center gap-2.5 px-2.5 py-[7px] rounded-lg text-[13.5px] no-underline transition-colors {isActive(item.href) ? 'bg-primary/10 text-primary font-medium' : 'text-base-content hover:bg-base-300'}"
				>
					<Icon class="w-4 h-4" strokeWidth={1.8} />
					<span>{item.label}</span>
				</a>
			{/each}

			<div class="h-px bg-base-300 my-2 mx-2.5"></div>

			{#each utilItems as item}
				{@const Icon = item.icon}
				<a
					href={item.href}
					class="flex items-center gap-2.5 px-2.5 py-[7px] rounded-lg text-[13.5px] no-underline transition-colors {isActive(item.href) ? 'bg-primary/10 text-primary font-medium' : 'text-base-content hover:bg-base-300'}"
				>
					<Icon class="w-4 h-4" strokeWidth={1.8} />
					<span>{item.label}</span>
				</a>
			{/each}
		</div>

		<div class="px-2.5 pb-2.5 pt-1 border-t border-base-300">
			<AvatarMenu {userName} />
		</div>
	</aside>

	<main class="flex-1 min-w-0 overflow-y-auto px-6 pt-6">
		{@render children()}
	</main>
</div>
