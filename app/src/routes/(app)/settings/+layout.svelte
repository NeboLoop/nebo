<script lang="ts">
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import {
		User,
		Key,
		Cpu,
		Sparkles,
		Brain,
		History,
		Activity,
		Shield,
		Fingerprint,
		ScrollText,
		StickyNote,
		MessagesSquare,
		Code,
		Cloud,
		CreditCard,
		BarChart3,
		Lock,
		X,
		ArrowUpCircle,
		Info
	} from 'lucide-svelte';
	import type { Snippet, Component } from 'svelte';
	import { updateInfo } from '$lib/stores/update';
	import { settingsReturnPath } from '$lib/stores/settings';

	let { children }: { children: Snippet } = $props();

	interface NavItem {
		id: string;
		path: string;
		label: string;
		icon: Component;
	}

	// Apple-style flat list — null entries create whitespace gaps
	const items: (NavItem | null)[] = [
		{ id: 'neboloop', path: '/settings/account', label: 'Account', icon: Cloud },
		{ id: 'profile', path: '/settings/profile', label: 'Profile', icon: User },
		{ id: 'billing', path: '/settings/billing', label: 'Billing', icon: CreditCard },
		{ id: 'usage', path: '/settings/usage', label: 'Usage', icon: BarChart3 },
		null,
		{ id: 'identity', path: '/settings/identity', label: 'Identity', icon: Fingerprint },
		{ id: 'personality', path: '/settings/personality', label: 'Soul', icon: Sparkles },
		{ id: 'rules', path: '/settings/rules', label: 'Rules', icon: ScrollText },
		{ id: 'notes', path: '/settings/notes', label: 'Notes', icon: StickyNote },
		{ id: 'advisors', path: '/settings/advisors', label: 'Advisors', icon: MessagesSquare },
		null,
		{ id: 'providers', path: '/settings/providers', label: 'Providers', icon: Key },
		{ id: 'routing', path: '/settings/routing', label: 'Routing', icon: Cpu },
		{ id: 'secrets', path: '/settings/secrets', label: 'Secrets', icon: Lock },
		null,
		{ id: 'permissions', path: '/settings/permissions', label: 'Permissions', icon: Shield },
		null,
		{ id: 'sessions', path: '/settings/sessions', label: 'Sessions', icon: History },
		{ id: 'memories', path: '/settings/memories', label: 'Memories', icon: Brain },
		{ id: 'status', path: '/settings/status', label: 'Status', icon: Activity },
		null,
		{ id: 'developer', path: '/settings/developer', label: 'Developer', icon: Code },
		null,
		{ id: 'about', path: '/settings/about', label: 'About', icon: Info },
	];

	const allTabs = $derived(items.filter((i): i is NavItem => i !== null));

	let activeTab = $derived(
		allTabs.find((t) => $page.url.pathname.startsWith(t.path))?.id || 'neboloop'
	);

	function closeSettings() {
		const returnTo = $settingsReturnPath || '/';
		settingsReturnPath.set('/');
		goto(returnTo);
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') closeSettings();
	}
</script>

<svelte:head>
	<title>Settings - Nebo</title>
	<meta name="description" content="Manage your account settings, preferences, and billing." />
</svelte:head>

<svelte:window onkeydown={handleKeydown} />

<!-- Settings modal overlay -->
<div class="fixed inset-0 z-[60] flex items-center justify-center p-4 sm:p-8">
	<!-- Backdrop (no click-to-close — settings is a workspace modal) -->
	<div class="absolute inset-0 bg-black/60 backdrop-blur-sm"></div>

	<!-- Modal card -->
	<div class="relative w-full max-w-4xl flex flex-col rounded-2xl bg-base-100 border border-base-content/10 shadow-2xl overflow-hidden" style="height: calc(100vh - 4rem);">
		<!-- Header -->
		<div class="flex items-center justify-between px-6 py-4 border-b border-base-content/10 shrink-0">
			<div class="flex items-center gap-3">
				<h1 class="font-display text-lg font-bold text-base-content">Settings</h1>
				{#if $updateInfo}
					<span class="text-sm text-base-content/60">Nebo {$updateInfo.currentVersion}</span>
					{#if $updateInfo.available}
						<a
							href={$updateInfo.releaseUrl}
							target="_blank"
							rel="noopener noreferrer"
							class="flex items-center gap-1 text-sm text-info hover:text-info/80 transition-colors"
						>
							<ArrowUpCircle class="w-3 h-3" />
							<span>{$updateInfo.latestVersion}</span>
						</a>
					{/if}
				{/if}
			</div>
			<button
				class="p-1.5 rounded-full hover:bg-base-content/10 transition-colors"
				onclick={closeSettings}
				aria-label="Close settings"
			>
				<X class="w-4 h-4 text-base-content/90" />
			</button>
		</div>

		<!-- Body: sidebar + content -->
		<div class="flex flex-1 min-h-0 overflow-hidden">
			<!-- Sidebar (always visible) -->
			<nav class="w-48 shrink-0 border-r border-base-content/10 overflow-y-auto py-3 px-2" aria-label="Settings navigation">
				{@render navItems()}
			</nav>

			<!-- Content -->
			<main class="flex-1 min-w-0 overflow-y-auto p-6">
				<div class="max-w-2xl">
					{@render children()}
				</div>
			</main>
		</div>
	</div>
</div>

{#snippet navItems()}
	<ul class="flex flex-col gap-0.5">
		{#each items as item}
			{#if item === null}
				<li class="h-3"></li>
			{:else}
				<li>
					<a
						href={item.path}
						class="w-full flex items-center gap-2.5 px-3 py-1.5 rounded-lg text-base text-left transition-colors whitespace-nowrap
							{activeTab === item.id
								? 'bg-primary/10 text-primary ring-1 ring-primary/20'
								: 'text-base-content/90 hover:bg-base-200 hover:text-base-content'}"
						aria-current={activeTab === item.id ? 'page' : undefined}
					>
						<item.icon class="w-4 h-4" />
						<span class="font-medium">{item.label}</span>
					</a>
				</li>
			{/if}
		{/each}
	</ul>
{/snippet}
