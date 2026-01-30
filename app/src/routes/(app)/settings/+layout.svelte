<script lang="ts">
	import { page } from '$app/stores';
	import { User, Settings, Bot, Key, Heart } from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();

	const tabs = [
		{ id: 'profile', path: '/settings/profile', label: 'Profile', icon: User },
		{ id: 'preferences', path: '/settings/preferences', label: 'Preferences', icon: Settings },
		{ id: 'providers', path: '/settings/providers', label: 'Providers', icon: Key },
		{ id: 'agent', path: '/settings/agent', label: 'Agent', icon: Bot },
		{ id: 'heartbeat', path: '/settings/heartbeat', label: 'Heartbeat', icon: Heart }
	] as const;

	// Determine active tab from URL path
	let activeTab = $derived(
		tabs.find((t) => $page.url.pathname.startsWith(t.path))?.id || 'profile'
	);

	// Heartbeat page needs full-height flex layout
	const isFullHeight = $derived($page.url.pathname.startsWith('/settings/heartbeat'));
</script>

<svelte:head>
	<title>Settings - GoBot</title>
	<meta name="description" content="Manage your account settings, preferences, and billing." />
</svelte:head>

<div class="flex flex-col {isFullHeight ? 'h-full min-h-0' : ''}">
	<div class="mb-8 shrink-0">
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Settings</h1>
		<p class="text-sm text-base-content/60">Manage your account and preferences</p>
	</div>

	<div class="flex flex-col lg:flex-row gap-6 {isFullHeight ? 'flex-1 min-h-0' : ''}">
		<!-- Sidebar Navigation -->
		<nav class="lg:w-56 flex-shrink-0" aria-label="Settings navigation">
			<ul class="flex lg:flex-col gap-1">
				{#each tabs as tab}
					<li>
						<a
							href={tab.path}
							class="w-full flex items-center gap-3 px-4 py-2.5 rounded-lg text-left transition-colors
								{activeTab === tab.id
									? 'bg-primary/10 text-primary border border-primary/20'
									: 'text-base-content/70 hover:bg-base-200 hover:text-base-content'}"
							aria-current={activeTab === tab.id ? 'page' : undefined}
						>
							<tab.icon class="w-5 h-5" />
							<span class="font-medium">{tab.label}</span>
						</a>
					</li>
				{/each}
			</ul>
		</nav>

		<!-- Content Area -->
		<main class="flex-1 min-w-0 {isFullHeight ? 'flex flex-col min-h-0' : ''}">
			{@render children()}
		</main>
	</div>
</div>
