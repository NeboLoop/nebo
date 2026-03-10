<!--
  Reusable Empty State Component
-->

<script lang="ts">
	import { Inbox, FileText, Users, Mail, Settings, Search } from 'lucide-svelte';
	import type { ComponentType, Snippet } from 'svelte';

	let {
		icon = 'inbox',
		title,
		message,
		description,
		children
	}: {
		icon?: string | ComponentType;
		title: string;
		message?: string;
		description?: string;
		children?: Snippet;
	} = $props();

	// Support 'description' as alias for 'message'
	const displayMessage = $derived(message || description);

	const iconMap: Record<string, ComponentType> = {
		inbox: Inbox,
		file: FileText,
		users: Users,
		mail: Mail,
		settings: Settings,
		search: Search
	};

	// Support both string icon names and direct Lucide components
	const Icon = $derived(
		typeof icon === 'string' ? (iconMap[icon] || Inbox) : icon
	);
</script>

<div class="p-12 text-center">
	<div class="mx-auto mb-4 flex h-16 w-16 items-center justify-center rounded-full bg-base-300">
		<Icon class="h-8 w-8 text-base-content/50" />
	</div>
	<p class="text-sm font-medium">{title}</p>
	{#if displayMessage}
		<p class="mt-1 text-xs text-base-content/60">{displayMessage}</p>
	{/if}
	{#if children}
		<div class="mt-4">
			{@render children()}
		</div>
	{/if}
</div>
