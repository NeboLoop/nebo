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
		Code,
		Package,
		BookOpen,
		Link,
		Users,
		Fingerprint,
		ScrollText,
		StickyNote
	} from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();

	const groups = [
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
				{ id: 'memories', path: '/settings/memories', label: 'Memories', icon: Brain }
			]
		},
		{
			label: 'Extend',
			tabs: [
				{ id: 'apps', path: '/settings/apps', label: 'Apps', icon: Package },
				{ id: 'skills', path: '/settings/skills', label: 'Skills', icon: BookOpen },
				{ id: 'integrations', path: '/settings/integrations', label: 'Integrations', icon: Link }
			]
		},
		{
			label: 'Family',
			tabs: [
				{ id: 'family', path: '/settings/family', label: 'Family', icon: Users }
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

</script>

<svelte:head>
	<title>Settings - Nebo</title>
	<meta name="description" content="Manage your account settings, preferences, and billing." />
</svelte:head>

<div class="flex flex-col">
	<div class="mb-8 shrink-0">
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Settings</h1>
		<p class="text-sm text-base-content/60">Manage your account and preferences</p>
	</div>

	<div class="flex flex-col lg:flex-row gap-6">
		<!-- Sidebar Navigation -->
		<nav class="lg:w-56 flex-shrink-0" aria-label="Settings navigation">
			<ul class="flex lg:flex-col gap-1">
				{#each groups as group, gi}
					{#if gi > 0}
						<li class="hidden lg:block h-3"></li>
					{/if}
					<li class="hidden lg:block">
						<span class="px-4 text-xs font-semibold uppercase tracking-wider text-base-content/40">
							{group.label}
						</span>
					</li>
					{#each group.tabs as tab}
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
				{/each}
			</ul>
		</nav>

		<!-- Content Area -->
		<main class="flex-1 min-w-0">
			{@render children()}
		</main>
	</div>
</div>
