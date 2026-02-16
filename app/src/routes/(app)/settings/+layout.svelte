<script lang="ts">
	import { page } from '$app/stores';
	import {
		User,
		Settings,
		Key,
		Heart,
		Sparkles,
		Brain,
		History,
		Activity,
		Shield,
		Package,
		BookOpen,
		Link,
		Fingerprint,
		ScrollText,
		StickyNote,
		MessagesSquare,
		Code,
		Cloud,
		Menu,
		X
	} from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();

	let drawerOpen = $state(false);

	const groups = [
		{
			label: 'Extend',
			tabs: [
				{ id: 'neboloop', path: '/settings/neboloop', label: 'NeboLoop', icon: Cloud },
				{ id: 'apps', path: '/settings/apps', label: 'Apps', icon: Package },
				{ id: 'skills', path: '/settings/skills', label: 'Skills', icon: BookOpen },
				{ id: 'integrations', path: '/settings/integrations', label: 'Integrations', icon: Link }
			]
		},
		{
			label: 'You',
			tabs: [
				{ id: 'profile', path: '/settings/profile', label: 'Profile', icon: User },
				{ id: 'preferences', path: '/settings/preferences', label: 'Preferences', icon: Settings }
			]
		},
		{
			label: 'Agent',
			tabs: [
				{ id: 'identity', path: '/settings/identity', label: 'Identity', icon: Fingerprint },
				{ id: 'soul', path: '/settings/personality', label: 'Soul', icon: Sparkles },
				{ id: 'rules', path: '/settings/rules', label: 'Rules', icon: ScrollText },
				{ id: 'notes', path: '/settings/notes', label: 'Notes', icon: StickyNote },
				{ id: 'providers', path: '/settings/providers', label: 'Providers', icon: Key },
				{ id: 'permissions', path: '/settings/permissions', label: 'Permissions', icon: Shield },
				{ id: 'heartbeat', path: '/settings/heartbeat', label: 'Heartbeat', icon: Heart },
				{ id: 'memories', path: '/settings/memories', label: 'Memories', icon: Brain },
				{ id: 'advisors', path: '/settings/advisors', label: 'Advisors', icon: MessagesSquare }
			]
		},
		{
			label: 'System',
			tabs: [
				{ id: 'sessions', path: '/settings/sessions', label: 'Sessions', icon: History },
				{ id: 'status', path: '/settings/status', label: 'Status', icon: Activity }
			]
		},
		{
			label: 'Developer',
			tabs: [
				{ id: 'developer', path: '/settings/developer', label: 'Developer', icon: Code }
			]
		}
	];

	const allTabs = $derived(groups.flatMap((g) => g.tabs));

	// Determine active tab from URL path
	let activeTab = $derived(
		allTabs.find((t) => $page.url.pathname.startsWith(t.path))?.id || 'profile'
	);

	let activeLabel = $derived(
		allTabs.find((t) => t.id === activeTab)?.label || 'Settings'
	);

	function closeDrawer() {
		drawerOpen = false;
	}
</script>

<svelte:head>
	<title>Settings - Nebo</title>
	<meta name="description" content="Manage your account settings, preferences, and billing." />
</svelte:head>

<div class="flex flex-col">
	<div class="mb-6 shrink-0 flex items-center gap-3">
		<button
			class="md:hidden btn btn-ghost btn-sm btn-square"
			onclick={() => (drawerOpen = true)}
			aria-label="Open settings menu"
		>
			<Menu class="w-5 h-5" />
		</button>
		<div>
			<h1 class="font-display text-2xl font-bold text-base-content mb-1">Settings</h1>
			<p class="text-sm text-base-content/60">Manage your account and preferences</p>
		</div>
	</div>

	<div class="flex flex-row gap-6">
		<!-- Sidebar Navigation (desktop) -->
		<nav class="hidden md:block w-48 flex-shrink-0" aria-label="Settings navigation">
			{@render navItems()}
		</nav>

		<!-- Content Area -->
		<main class="flex-1 min-w-0">
			{@render children()}
		</main>
	</div>
</div>

<!-- Mobile drawer overlay -->
{#if drawerOpen}
	<div class="fixed inset-0 z-50 md:hidden">
		<!-- Backdrop -->
		<button
			class="absolute inset-0 bg-black/40"
			onclick={closeDrawer}
			aria-label="Close settings menu"
		></button>
		<!-- Drawer panel -->
		<nav class="absolute inset-y-0 left-0 w-64 bg-base-100 shadow-xl p-4 overflow-y-auto" aria-label="Settings navigation">
			<div class="flex items-center justify-between mb-4">
				<span class="font-semibold text-base-content">Settings</span>
				<button class="btn btn-ghost btn-sm btn-square" onclick={closeDrawer} aria-label="Close">
					<X class="w-4 h-4" />
				</button>
			</div>
			{@render navItems()}
		</nav>
	</div>
{/if}

{#snippet navItems()}
	<ul class="flex flex-col gap-1">
		{#each groups as group, gi}
			{#if gi > 0}
				<li class="h-3"></li>
			{/if}
			<li>
				<span class="px-3 text-xs font-semibold uppercase tracking-wider text-base-content/40">
					{group.label}
				</span>
			</li>
			{#each group.tabs as tab}
				<li>
					<a
						href={tab.path}
						onclick={closeDrawer}
						class="w-full flex items-center gap-2.5 px-3 py-2 rounded-lg text-sm text-left transition-colors whitespace-nowrap
							{activeTab === tab.id
								? 'bg-primary/10 text-primary border border-primary/20'
								: 'text-base-content/70 hover:bg-base-200 hover:text-base-content border border-transparent'}"
						aria-current={activeTab === tab.id ? 'page' : undefined}
					>
						<tab.icon class="w-4 h-4" />
						<span class="font-medium">{tab.label}</span>
					</a>
				</li>
			{/each}
		{/each}
	</ul>
{/snippet}
