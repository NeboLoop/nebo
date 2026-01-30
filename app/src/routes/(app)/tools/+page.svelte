<script lang="ts">
	import { onMount } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Wrench, Shield, ShieldAlert, RefreshCw, Terminal, FileText, Globe, Search, Cpu, Zap, Plug, MessageSquare, Power } from 'lucide-svelte';
	import type { ListExtensionsResponse, ExtensionTool, ExtensionSkill, ExtensionChannel } from '$lib/api/neboComponents';

	let extensions = $state<ListExtensionsResponse | null>(null);
	let isLoading = $state(true);
	let togglingSkill = $state<string | null>(null);
	let activeTab = $state<'tools' | 'skills' | 'plugins'>('tools');
	let toolFilter = $state<'all' | 'builtin' | 'plugins'>('all');

	const categoryIcons: Record<string, any> = {
		'shell': Terminal,
		'file': FileText,
		'web': Globe,
		'search': Search,
		'process': Cpu,
		'default': Wrench
	};

	onMount(async () => {
		await loadExtensions();
	});

	async function loadExtensions() {
		isLoading = true;
		try {
			const response = await fetch('/api/v1/extensions');
			if (response.ok) {
				extensions = await response.json();
			}
		} catch (error) {
			console.error('Failed to load extensions:', error);
		} finally {
			isLoading = false;
		}
	}

	const filteredTools = $derived(() => {
		if (!extensions?.tools) return [];
		if (toolFilter === 'all') return extensions.tools;
		if (toolFilter === 'builtin') return extensions.tools.filter(t => !t.isPlugin);
		return extensions.tools.filter(t => t.isPlugin);
	});

	function getIcon(name: string) {
		if (name === 'bash') return Terminal;
		if (['read', 'write', 'edit', 'glob'].includes(name)) return FileText;
		if (['web', 'browser'].includes(name)) return Globe;
		if (['grep', 'search'].includes(name)) return Search;
		if (['process', 'task'].includes(name)) return Cpu;
		return Wrench;
	}

	async function toggleSkill(name: string) {
		togglingSkill = name;
		try {
			const response = await fetch(`/api/v1/skills/${encodeURIComponent(name)}/toggle`, {
				method: 'POST'
			});
			if (response.ok) {
				// Refresh the extensions list to get updated state
				await loadExtensions();
			} else {
				console.error('Failed to toggle skill:', await response.text());
			}
		} catch (error) {
			console.error('Failed to toggle skill:', error);
		} finally {
			togglingSkill = null;
		}
	}
</script>

<svelte:head>
	<title>Extensions - Nebo</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h1 class="font-display text-2xl font-bold text-base-content mb-1">Extensions</h1>
		<p class="text-sm text-base-content/60">Tools, skills, and plugins for the AI agent</p>
	</div>
	<Button type="ghost" onclick={loadExtensions}>
		<RefreshCw class="w-4 h-4 mr-2" />
		Refresh
	</Button>
</div>

<!-- Main Tabs -->
<div class="tabs tabs-boxed bg-base-200 mb-6 p-1 w-fit">
	<button
		class="tab gap-2 {activeTab === 'tools' ? 'tab-active bg-base-100' : ''}"
		onclick={() => activeTab = 'tools'}
	>
		<Wrench class="w-4 h-4" />
		Tools
		{#if extensions?.tools}
			<span class="badge badge-sm badge-ghost">{extensions.tools.length}</span>
		{/if}
	</button>
	<button
		class="tab gap-2 {activeTab === 'skills' ? 'tab-active bg-base-100' : ''}"
		onclick={() => activeTab = 'skills'}
	>
		<Zap class="w-4 h-4" />
		Skills
		{#if extensions?.skills}
			<span class="badge badge-sm badge-ghost">{extensions.skills.length}</span>
		{/if}
	</button>
	<button
		class="tab gap-2 {activeTab === 'plugins' ? 'tab-active bg-base-100' : ''}"
		onclick={() => activeTab = 'plugins'}
	>
		<Plug class="w-4 h-4" />
		Channels
		{#if extensions?.channels}
			<span class="badge badge-sm badge-ghost">{extensions.channels.length}</span>
		{/if}
	</button>
</div>

{#if isLoading}
	<Card>
		<div class="py-12 text-center text-base-content/60">Loading extensions...</div>
	</Card>
{:else if activeTab === 'tools'}
	<!-- Tool Filter -->
	<div class="flex gap-2 mb-6">
		<button
			onclick={() => toolFilter = 'all'}
			class="px-4 py-2 rounded-lg text-sm font-medium transition-colors {toolFilter === 'all' ? 'bg-primary text-primary-content' : 'bg-base-200 hover:bg-base-300'}"
		>
			All ({extensions?.tools?.length || 0})
		</button>
		<button
			onclick={() => toolFilter = 'builtin'}
			class="px-4 py-2 rounded-lg text-sm font-medium transition-colors flex items-center gap-2 {toolFilter === 'builtin' ? 'bg-info text-info-content' : 'bg-base-200 hover:bg-base-300'}"
		>
			<Wrench class="w-4 h-4" />
			Built-in ({extensions?.tools?.filter(t => !t.isPlugin).length || 0})
		</button>
		<button
			onclick={() => toolFilter = 'plugins'}
			class="px-4 py-2 rounded-lg text-sm font-medium transition-colors flex items-center gap-2 {toolFilter === 'plugins' ? 'bg-secondary text-secondary-content' : 'bg-base-200 hover:bg-base-300'}"
		>
			<Plug class="w-4 h-4" />
			Plugins ({extensions?.tools?.filter(t => t.isPlugin).length || 0})
		</button>
	</div>

	<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
		{#each filteredTools() as tool}
			{@const Icon = getIcon(tool.name)}
			<Card class="hover:border-primary/30 transition-colors">
				<div class="flex items-start gap-3">
					<div class="w-10 h-10 rounded-xl {tool.requiresApproval ? 'bg-warning/10' : 'bg-success/10'} flex items-center justify-center shrink-0">
						<Icon class="w-5 h-5 {tool.requiresApproval ? 'text-warning' : 'text-success'}" />
					</div>
					<div class="flex-1 min-w-0">
						<div class="flex items-center gap-2 mb-1">
							<h3 class="font-display font-bold text-base-content">{tool.name}</h3>
							{#if tool.isPlugin}
								<span class="px-2 py-0.5 rounded text-xs bg-secondary/20 text-secondary">
									Plugin
								</span>
							{/if}
							{#if tool.requiresApproval}
								<span class="px-2 py-0.5 rounded text-xs bg-warning/20 text-warning">
									Approval
								</span>
							{/if}
						</div>
						<p class="text-sm text-base-content/60">{tool.description}</p>
					</div>
				</div>
			</Card>
		{/each}
	</div>

{:else if activeTab === 'skills'}
	{#if extensions?.skills && extensions.skills.length > 0}
		<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
			{#each extensions.skills as skill}
				<Card class="hover:border-primary/30 transition-colors">
					<div class="flex items-start gap-3">
						<div class="w-10 h-10 rounded-xl {skill.enabled ? 'bg-primary/10' : 'bg-base-200'} flex items-center justify-center shrink-0">
							<Zap class="w-5 h-5 {skill.enabled ? 'text-primary' : 'text-base-content/30'}" />
						</div>
						<div class="flex-1 min-w-0">
							<div class="flex items-center justify-between gap-2 mb-1">
								<div class="flex items-center gap-2">
									<h3 class="font-display font-bold text-base-content">{skill.name}</h3>
									<span class="px-2 py-0.5 rounded text-xs bg-base-200">
										v{skill.version}
									</span>
								</div>
								<button
									class="btn btn-xs btn-ghost {skill.enabled ? 'text-success' : 'text-base-content/40'}"
									onclick={() => toggleSkill(skill.name)}
									disabled={togglingSkill === skill.name}
									title={skill.enabled ? 'Click to disable' : 'Click to enable'}
								>
									{#if togglingSkill === skill.name}
										<span class="loading loading-spinner loading-xs"></span>
									{:else}
										<Power class="w-4 h-4" />
									{/if}
								</button>
							</div>
							<p class="text-sm text-base-content/60 mb-2 {!skill.enabled ? 'opacity-50' : ''}">{skill.description}</p>

							{#if skill.triggers && skill.triggers.length > 0}
								<div class="flex flex-wrap gap-1 mb-2 {!skill.enabled ? 'opacity-50' : ''}">
									{#each skill.triggers.slice(0, 3) as trigger}
										<span class="badge badge-sm badge-outline">{trigger}</span>
									{/each}
									{#if skill.triggers.length > 3}
										<span class="badge badge-sm badge-ghost">+{skill.triggers.length - 3}</span>
									{/if}
								</div>
							{/if}

							{#if skill.tools && skill.tools.length > 0}
								<div class="text-xs text-base-content/50 {!skill.enabled ? 'opacity-50' : ''}">
									Uses: {skill.tools.join(', ')}
								</div>
							{/if}
						</div>
					</div>
				</Card>
			{/each}
		</div>
	{:else}
		<Card>
			<div class="py-12 text-center text-base-content/60">
				<Zap class="w-12 h-12 mx-auto mb-4 opacity-20" />
				<p class="font-medium mb-2">No skills found</p>
				<p class="text-sm">Add YAML skill files to <code class="bg-base-200 px-2 py-1 rounded">extensions/skills/</code></p>
			</div>
		</Card>
	{/if}

{:else if activeTab === 'plugins'}
	{#if extensions?.channels && extensions.channels.length > 0}
		<div class="grid sm:grid-cols-2 lg:grid-cols-3 gap-4">
			{#each extensions.channels as channel}
				<Card class="hover:border-primary/30 transition-colors">
					<div class="flex items-start gap-3">
						<div class="w-10 h-10 rounded-xl bg-accent/10 flex items-center justify-center shrink-0">
							<MessageSquare class="w-5 h-5 text-accent" />
						</div>
						<div class="flex-1 min-w-0">
							<h3 class="font-display font-bold text-base-content mb-1">{channel.id}</h3>
							<p class="text-sm text-base-content/60 truncate">{channel.path}</p>
						</div>
					</div>
				</Card>
			{/each}
		</div>
	{:else}
		<Card>
			<div class="py-12 text-center text-base-content/60">
				<Plug class="w-12 h-12 mx-auto mb-4 opacity-20" />
				<p class="font-medium mb-2">No channel plugins found</p>
				<p class="text-sm">Add plugin executables to <code class="bg-base-200 px-2 py-1 rounded">extensions/plugins/channels/</code></p>
			</div>
		</Card>
	{/if}
{/if}
