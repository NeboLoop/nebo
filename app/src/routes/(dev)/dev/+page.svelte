<script lang="ts">
	import { onMount } from 'svelte';
	import { Eye, Terminal, Wrench, Activity, Play, RotateCcw, FolderOpen, Loader2 } from 'lucide-svelte';
	import DevChatPanel from '$lib/components/dev/DevChatPanel.svelte';
	import AppLogs from '$lib/components/dev/AppLogs.svelte';
	import ToolTester from '$lib/components/dev/ToolTester.svelte';
	import GrpcInspector from '$lib/components/dev/GrpcInspector.svelte';
	import Tabs from '$lib/components/ui/Tabs.svelte';
	import { DEV_ASSISTANT_PROMPT } from '$lib/components/dev/devPrompt';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';

	const tabs = [
		{ id: 'preview', label: 'Preview', icon: Eye },
		{ id: 'logs', label: 'Logs', icon: Terminal },
		{ id: 'grpc', label: 'gRPC', icon: Activity },
		{ id: 'tester', label: 'Tools', icon: Wrench }
	];

	let activeTab = $state('preview');
	let devApps = $state<components.DevAppItem[]>([]);
	let selectedAppId = $state('');
	let building = $state(false);
	let sideloading = $state(false);

	// Derive session key from selected project for per-project chat persistence
	const devSessionKey = $derived(
		selectedAppId ? `dev-${selectedAppId}` : 'dev-general'
	);

	// Derive selected app details
	const selectedApp = $derived(devApps.find(a => a.appId === selectedAppId));

	// Full project context for the Dev Assistant
	let projectCtx = $state<components.ProjectContext | null>(null);

	// Fetch project context whenever selected app changes
	$effect(() => {
		const id = selectedAppId;
		if (!id) {
			projectCtx = null;
			return;
		}
		api.projectContext(id).then(ctx => {
			projectCtx = ctx;
		}).catch(() => {
			projectCtx = null;
		});
	});

	// Build system prompt with full project context
	const systemPrompt = $derived.by(() => {
		let prompt = DEV_ASSISTANT_PROMPT;
		const ctx = projectCtx;
		if (!ctx) return prompt;

		prompt += `\n\n## Current Project\n`;
		prompt += `\nDirectory: \`${ctx.path}\`\n`;
		if (ctx.appId) prompt += `App ID: ${ctx.appId}\n`;
		if (ctx.name) prompt += `Name: ${ctx.name}\n`;
		if (ctx.version) prompt += `Version: ${ctx.version}\n`;
		prompt += `Status: ${ctx.running ? '**Running**' : 'Not running'}\n`;
		prompt += `Has Makefile: ${ctx.hasMakefile ? 'Yes' : 'No'}\n`;
		if (ctx.binaryPath) prompt += `Binary: \`${ctx.binaryPath}\`\n`;

		if (ctx.files?.length) {
			prompt += `\n### Project Files\n\`\`\`\n${ctx.files.join('\n')}\n\`\`\`\n`;
		}

		if (ctx.manifestRaw) {
			prompt += `\n### manifest.json\n\`\`\`json\n${ctx.manifestRaw}\n\`\`\`\n`;
		}

		if (ctx.recentLogs) {
			prompt += `\n### Recent Logs\n\`\`\`\n${ctx.recentLogs}\n\`\`\`\n`;
		}

		prompt += `\nAll file operations MUST target \`${ctx.path}\`. `;
		prompt += `When scaffolding, create files in this directory. `;
		prompt += `When building, run \`make build\` in this directory. `;
		prompt += `When reading logs, check \`${ctx.path}/logs/\`.`;

		return prompt;
	});

	// --- Resizable split ---
	const SPLIT_KEY = 'nebo-dev-split';
	const DEFAULT_SPLIT = 50;
	const MIN_SPLIT = 20;
	const MAX_SPLIT = 80;

	let splitPct = $state(DEFAULT_SPLIT);
	let isResizing = $state(false);
	let containerEl: HTMLDivElement;

	onMount(() => {
		// Restore split position
		const savedSplit = localStorage.getItem(SPLIT_KEY);
		if (savedSplit) {
			const n = parseFloat(savedSplit);
			if (n >= MIN_SPLIT && n <= MAX_SPLIT) splitPct = n;
		}

		// Restore tab
		const savedTab = localStorage.getItem('nebo-dev-active-tab');
		if (savedTab && tabs.some(t => t.id === savedTab)) {
			activeTab = savedTab;
		}

		// Restore selected app
		const savedApp = localStorage.getItem('nebo-dev-selected-app');
		if (savedApp) selectedAppId = savedApp;

		loadDevApps();
	});

	// Persist split position
	$effect(() => {
		localStorage.setItem(SPLIT_KEY, String(splitPct));
	});

	// Persist active tab
	$effect(() => {
		localStorage.setItem('nebo-dev-active-tab', activeTab);
	});

	// Persist selected app
	$effect(() => {
		if (selectedAppId) {
			localStorage.setItem('nebo-dev-selected-app', selectedAppId);
		}
	});

	function startResize(e: PointerEvent) {
		isResizing = true;
		(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
	}

	function onResize(e: PointerEvent) {
		if (!isResizing || !containerEl) return;
		const rect = containerEl.getBoundingClientRect();
		const pct = ((e.clientX - rect.left) / rect.width) * 100;
		splitPct = Math.min(MAX_SPLIT, Math.max(MIN_SPLIT, pct));
	}

	function stopResize() {
		isResizing = false;
	}

	async function loadDevApps() {
		try {
			const res = await api.listDevApps();
			devApps = res.apps ?? [];
			// Auto-select first app if none selected or saved selection not found
			if (devApps.length > 0 && !devApps.find(a => a.appId === selectedAppId)) {
				selectedAppId = devApps[0].appId;
			}
		} catch {
			// ignore — apps may not be available yet
		}
	}

	let actionError = $state('');

	async function refreshContext() {
		if (!selectedAppId) return;
		try {
			projectCtx = await api.projectContext(selectedAppId);
		} catch {
			projectCtx = null;
		}
	}

	async function handleBuildRun() {
		if (!selectedAppId || building) return;
		actionError = '';
		building = true;
		try {
			const res = await api.relaunchDevApp(selectedAppId);
			// Update app ID if manifest changed (e.g. first build after scaffolding)
			if (res.appId && res.appId !== selectedAppId) {
				selectedAppId = res.appId;
			}
			await loadDevApps();
			await refreshContext();
		} catch (err: any) {
			actionError = err.message || 'Build failed';
			await refreshContext(); // Refresh context even on error — shows logs
		} finally {
			building = false;
		}
	}

	async function handleSideload() {
		if (sideloading) return;
		actionError = '';
		sideloading = true;
		try {
			// Open native directory picker
			const browse = await api.browseDirectory();
			if (!browse.path) {
				// User cancelled
				return;
			}
			// Add project to dev workspace (no build, no launch)
			const res = await api.sideload({ path: browse.path });
			await loadDevApps();
			if (res.appId) {
				selectedAppId = res.appId;
			}
		} catch (err: any) {
			actionError = err.message || 'Failed to add project';
		} finally {
			sideloading = false;
		}
	}

	const devAssistantSuggestions = [
		'Scaffold a new Nebo tool app',
		'Help me debug my app — it won\'t start',
		'Explain the Nebo app manifest format',
		'Run a build and check for errors'
	];
</script>

<div class="flex flex-col h-full min-h-0">
	<!-- Project Header Bar -->
	<div class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-base-300 bg-base-200/50">
		<span class="text-xs font-medium text-base-content/50">Project:</span>
		<select
			bind:value={selectedAppId}
			class="select select-bordered select-xs"
		>
			{#if devApps.length === 0}
				<option value="">No projects</option>
			{/if}
			{#each devApps as app}
				<option value={app.appId}>{app.name || app.appId}</option>
			{/each}
		</select>

		<button
			type="button"
			class="btn btn-xs btn-ghost"
			onclick={handleBuildRun}
			disabled={!selectedAppId || building}
			title={selectedApp?.running ? 'Rebuild & Relaunch' : 'Build & Run'}
		>
			{#if building}
				<Loader2 class="w-3.5 h-3.5 animate-spin" />
			{:else if selectedApp?.running}
				<RotateCcw class="w-3.5 h-3.5" />
			{:else}
				<Play class="w-3.5 h-3.5" />
			{/if}
		</button>

		<button
			type="button"
			class="btn btn-xs btn-ghost"
			onclick={handleSideload}
			disabled={sideloading}
			title="Open project folder"
		>
			{#if sideloading}
				<Loader2 class="w-3.5 h-3.5 animate-spin" />
			{:else}
				<FolderOpen class="w-3.5 h-3.5" />
			{/if}
		</button>

		{#if actionError}
			<span class="text-xs text-error ml-auto truncate max-w-xs" title={actionError}>
				{actionError}
			</span>
		{:else if selectedApp}
			<span class="text-xs text-base-content/30 ml-auto">
				{selectedApp.running ? 'Running' : 'Stopped'}
			</span>
		{/if}
	</div>

	<!-- Main Split Layout -->
	<div class="flex flex-1 min-h-0" bind:this={containerEl}>
		<!-- Left Panel: Dev Assistant Chat (per-project) -->
		<div class="flex flex-col min-h-0 overflow-hidden" style:flex-basis="{splitPct}%">
			{#key devSessionKey}
				<DevChatPanel
					sessionKey={devSessionKey}
					{systemPrompt}
					title="Dev Assistant"
					subtitle={selectedApp ? selectedApp.name || selectedApp.appId : 'AI pair programmer for Nebo apps'}
					suggestions={devAssistantSuggestions}
				/>
			{/key}
		</div>

		<!-- Resizer -->
		<div
			class="w-2 cursor-col-resize shrink-0 relative z-10 group select-none touch-none"
			role="separator"
			aria-orientation="vertical"
			aria-valuenow={Math.round(splitPct)}
			onpointerdown={startResize}
			onpointermove={onResize}
			onpointerup={stopResize}
			onlostpointercapture={stopResize}
		>
			<div class="absolute inset-y-0 left-1/2 -translate-x-1/2 transition-all
				{isResizing ? 'w-[3px] bg-primary' : 'w-px bg-base-content/15 group-hover:w-[3px] group-hover:bg-primary/50'}
			"></div>
		</div>

		<!-- Right Panel: Tabbed Inspector -->
		<div class="flex flex-col min-h-0 overflow-hidden" style:flex-basis="{100 - splitPct}%">
			<!-- Tab Bar -->
			<div class="shrink-0 border-b border-base-300 px-2 pt-1">
				<Tabs {tabs} bind:activeTab variant="underline" />
			</div>

			<!-- Tab Content -->
			<div class="flex-1 min-h-0 overflow-hidden">
				{#if activeTab === 'preview'}
					{#if selectedAppId}
						<iframe
							src="/api/v1/apps/{selectedAppId}/ui/"
							class="w-full h-full border-0"
							title="App UI Preview"
							sandbox="allow-scripts allow-forms allow-same-origin"
						></iframe>
					{:else}
						<div class="flex items-center justify-center h-full text-base-content/50 text-sm">
							Select an app to preview its UI
						</div>
					{/if}
				{:else if activeTab === 'logs'}
					<AppLogs appId={selectedAppId} />
				{:else if activeTab === 'grpc'}
					<GrpcInspector appId={selectedAppId} />
				{:else if activeTab === 'tester'}
					<ToolTester appId={selectedAppId} />
				{/if}
			</div>
		</div>
	</div>
</div>
