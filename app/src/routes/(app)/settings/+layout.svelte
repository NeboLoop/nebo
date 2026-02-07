<script lang="ts">
	import { page } from '$app/stores';
	import {
		User,
		Settings,
		Bot,
		Key,
		Heart,
		Sparkles,
		UserCircle,
		Brain,
		History,
		Wrench,
		MessageCircle,
		Server,
		Activity,
		Puzzle,
		Shield
	} from 'lucide-svelte';
	import type { Snippet } from 'svelte';

	let { children }: { children: Snippet } = $props();

	const groups = [
		{
			label: 'Your Account',
			tabs: [
				{ id: 'profile', path: '/settings/profile', label: 'Profile', icon: User },
				{ id: 'about-me', path: '/settings/about-me', label: 'About Me', icon: UserCircle },
				{ id: 'preferences', path: '/settings/preferences', label: 'Preferences', icon: Settings }
			]
		},
		{
			label: 'Agent',
			tabs: [
				{ id: 'personality', path: '/settings/personality', label: 'Personality', icon: Sparkles },
				{ id: 'providers', path: '/settings/providers', label: 'Providers', icon: Key },
				{ id: 'permissions', path: '/settings/permissions', label: 'Permissions', icon: Shield },
				{ id: 'agent', path: '/settings/agent', label: 'Agent Config', icon: Bot },
				{ id: 'heartbeat', path: '/settings/heartbeat', label: 'Heartbeat', icon: Heart },
				{ id: 'memories', path: '/settings/memories', label: 'Memories', icon: Brain }
			]
		},
		{
			label: 'System',
			tabs: [
				{ id: 'sessions', path: '/settings/sessions', label: 'Sessions', icon: History },
				{ id: 'extensions', path: '/settings/extensions', label: 'Extensions', icon: Wrench },
				{ id: 'plugins', path: '/settings/plugins', label: 'Plugins', icon: Puzzle },
				{ id: 'channels', path: '/settings/channels', label: 'Channels', icon: MessageCircle },
				{ id: 'mcp', path: '/settings/mcp', label: 'MCP', icon: Server },
				{ id: 'status', path: '/settings/status', label: 'Status', icon: Activity }
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
