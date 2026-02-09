<script lang="ts">
	import { onMount } from 'svelte';
	import { Eye, Terminal, Search, Wrench, Settings, Upload } from 'lucide-svelte';
	import DevChatPanel from '$lib/components/dev/DevChatPanel.svelte';
	import AppPreviewChat from '$lib/components/dev/AppPreviewChat.svelte';
	import AppLogs from '$lib/components/dev/AppLogs.svelte';
	import Tabs from '$lib/components/ui/Tabs.svelte';
	import { DEV_ASSISTANT_PROMPT } from '$lib/components/dev/devPrompt';
	import * as api from '$lib/api/nebo';
	import type * as components from '$lib/api/neboComponents';

	const tabs = [
		{ id: 'preview', label: 'Preview', icon: Eye },
		{ id: 'logs', label: 'Logs', icon: Terminal },
		{ id: 'inspector', label: 'Inspector', icon: Search },
		{ id: 'tester', label: 'Tool Tester', icon: Wrench },
		{ id: 'settings', label: 'Settings', icon: Settings },
		{ id: 'submit', label: 'Submit', icon: Upload }
	];

	let activeTab = $state('preview');
	let devApps = $state<components.DevAppItem[]>([]);

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
		} catch {
			// ignore — apps may not be available yet
		}
	}

	const devAssistantSuggestions = [
		'Scaffold a new Nebo tool app',
		'Help me debug my app — it won\'t start',
		'Explain the Nebo app manifest format',
		'Run a build and check for errors'
	];
</script>

<div class="flex h-full min-h-0" bind:this={containerEl}>
	<!-- Left Panel: Dev Assistant Chat -->
	<div class="flex flex-col min-h-0 overflow-hidden" style:flex-basis="{splitPct}%">
		<DevChatPanel
			sessionKey="dev-assistant"
			systemPrompt={DEV_ASSISTANT_PROMPT}
			title="Dev Assistant"
			subtitle="AI pair programmer for Nebo apps"
			suggestions={devAssistantSuggestions}
		/>
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
				<AppPreviewChat {devApps} />
			{:else if activeTab === 'logs'}
				<AppLogs {devApps} />
			{:else if activeTab === 'inspector'}
				<div class="flex flex-col items-center justify-center h-full text-base-content/50 gap-2">
					<Search class="w-8 h-8" />
					<p class="text-sm font-medium">gRPC Inspector</p>
					<p class="text-xs">Coming soon — live gRPC traffic logging</p>
				</div>
			{:else if activeTab === 'tester'}
				<div class="flex flex-col items-center justify-center h-full text-base-content/50 gap-2">
					<Wrench class="w-8 h-8" />
					<p class="text-sm font-medium">Tool Tester</p>
					<p class="text-xs">Coming soon — execute tools directly</p>
				</div>
			{:else if activeTab === 'settings'}
				<div class="flex flex-col items-center justify-center h-full text-base-content/50 gap-2">
					<Settings class="w-8 h-8" />
					<p class="text-sm font-medium">App Settings</p>
					<p class="text-xs">Coming soon — test app configuration</p>
				</div>
			{:else if activeTab === 'submit'}
				<div class="flex flex-col items-center justify-center h-full text-base-content/50 gap-2">
					<Upload class="w-8 h-8" />
					<p class="text-sm font-medium">Submit to Marketplace</p>
					<p class="text-xs">Coming soon — package and publish your app</p>
				</div>
			{/if}
		</div>
	</div>
</div>
