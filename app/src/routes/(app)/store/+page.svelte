<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import Card from '$lib/components/ui/Card.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import Badge from '$lib/components/ui/Badge.svelte';
	import {
		RefreshCw,
		Search,
		Download,
		Trash2,
		Star,
		Package,
		Zap,
		BadgeCheck,
		Loader2,
		Link,
		Wifi,
		WifiOff
	} from 'lucide-svelte';
	import {
		listStoreApps,
		listStoreSkills,
		installStoreApp,
		uninstallStoreApp,
		installStoreSkill,
		uninstallStoreSkill,
		neboLoopStatus as fetchNeboLoopStatus,
		neboLoopConnect
	} from '$lib/api/index';
	import type { StoreApp, StoreSkill, NeboLoopStatusResponse } from '$lib/api';
	import { getWebSocketClient } from '$lib/websocket/client';

	let storeTab = $state<'apps' | 'skills'>('apps');

	// Store state - Apps
	let storeApps = $state<StoreApp[]>([]);
	let appsLoading = $state(false);
	let appsError = $state<string | null>(null);
	let appsLoaded = $state(false);
	let appsTotalCount = $state(0);
	let appsPage = $state(1);
	let appsPageSize = $state(20);

	// Store state - Skills
	let storeSkills = $state<StoreSkill[]>([]);
	let skillsLoading = $state(false);
	let skillsError = $state<string | null>(null);
	let skillsLoaded = $state(false);
	let skillsTotalCount = $state(0);
	let skillsPage = $state(1);
	let skillsPageSize = $state(20);

	// Shared store search/filter
	let storeSearch = $state('');
	let storeCategory = $state('');

	// Install/uninstall in-flight tracking
	let installing = $state<Record<string, boolean>>({});
	let uninstalling = $state<Record<string, boolean>>({});

	// NeboLoop connection state
	let neboLoopStatus = $state<NeboLoopStatusResponse | null>(null);
	let showConnectModal = $state(false);
	let connectCode = $state('');
	let connectName = $state('');
	let isConnectingNeboLoop = $state(false);
	let connectError = $state<string | null>(null);

	let unsubscribers: (() => void)[] = [];

	onMount(async () => {
		await loadNeboLoopStatus();
		loadCurrentStoreTab();

		const client = getWebSocketClient();
		unsubscribers.push(
			client.on('plugin_settings_updated', () => {
				loadNeboLoopStatus();
			})
		);
	});

	onDestroy(() => {
		unsubscribers.forEach((unsub) => unsub());
	});

	async function loadStoreApps() {
		appsLoading = true;
		appsError = null;
		try {
			const params: Record<string, string | number> = {
				page: appsPage,
				pageSize: appsPageSize
			};
			if (storeSearch) params.q = storeSearch;
			if (storeCategory) params.category = storeCategory;

			const data = await listStoreApps(params);
			storeApps = data.apps || [];
			appsTotalCount = data.totalCount || 0;
			appsLoaded = true;
		} catch (error: any) {
			appsError = error.message || 'Failed to load apps';
			storeApps = [];
		} finally {
			appsLoading = false;
		}
	}

	async function loadStoreSkills() {
		skillsLoading = true;
		skillsError = null;
		try {
			const params: Record<string, string | number> = {
				page: skillsPage,
				pageSize: skillsPageSize
			};
			if (storeSearch) params.q = storeSearch;
			if (storeCategory) params.category = storeCategory;

			const data = await listStoreSkills(params);
			storeSkills = data.skills || [];
			skillsTotalCount = data.totalCount || 0;
			skillsLoaded = true;
		} catch (error: any) {
			skillsError = error.message || 'Failed to load skills';
			storeSkills = [];
		} finally {
			skillsLoading = false;
		}
	}

	function switchStoreTab(tab: 'apps' | 'skills') {
		storeTab = tab;
		loadCurrentStoreTab();
	}

	function loadCurrentStoreTab() {
		if (storeTab === 'apps' && !appsLoaded) {
			loadStoreApps();
		} else if (storeTab === 'skills' && !skillsLoaded) {
			loadStoreSkills();
		}
	}

	function handleStoreSearch() {
		appsPage = 1;
		skillsPage = 1;
		appsLoaded = false;
		skillsLoaded = false;
		if (storeTab === 'apps') {
			loadStoreApps();
		} else {
			loadStoreSkills();
		}
	}

	function refreshStore() {
		appsLoaded = false;
		skillsLoaded = false;
		if (storeTab === 'apps') {
			loadStoreApps();
		} else {
			loadStoreSkills();
		}
	}

	async function handleInstallApp(app: StoreApp) {
		installing = { ...installing, [app.id]: true };
		try {
			await installStoreApp(app.id);
			storeApps = storeApps.map((a) => (a.id === app.id ? { ...a, isInstalled: true } : a));
		} catch (error: any) {
			console.error('Failed to install app:', error);
		} finally {
			installing = { ...installing, [app.id]: false };
		}
	}

	async function handleUninstallApp(app: StoreApp) {
		uninstalling = { ...uninstalling, [app.id]: true };
		try {
			await uninstallStoreApp(app.id);
			storeApps = storeApps.map((a) => (a.id === app.id ? { ...a, isInstalled: false } : a));
		} catch (error: any) {
			console.error('Failed to uninstall app:', error);
		} finally {
			uninstalling = { ...uninstalling, [app.id]: false };
		}
	}

	async function handleInstallSkill(skill: StoreSkill) {
		installing = { ...installing, [skill.id]: true };
		try {
			await installStoreSkill(skill.id);
			storeSkills = storeSkills.map((s) => (s.id === skill.id ? { ...s, isInstalled: true } : s));
		} catch (error: any) {
			console.error('Failed to install skill:', error);
		} finally {
			installing = { ...installing, [skill.id]: false };
		}
	}

	async function handleUninstallSkill(skill: StoreSkill) {
		uninstalling = { ...uninstalling, [skill.id]: true };
		try {
			await uninstallStoreSkill(skill.id);
			storeSkills = storeSkills.map((s) => (s.id === skill.id ? { ...s, isInstalled: false } : s));
		} catch (error: any) {
			console.error('Failed to uninstall skill:', error);
		} finally {
			uninstalling = { ...uninstalling, [skill.id]: false };
		}
	}

	async function loadNeboLoopStatus() {
		try {
			neboLoopStatus = await fetchNeboLoopStatus();
		} catch {
			neboLoopStatus = null;
		}
	}

	function openConnectModal() {
		connectCode = '';
		connectName = '';
		connectError = null;
		showConnectModal = true;
	}

	async function handleConnect() {
		if (!connectCode.trim() || !connectName.trim()) return;
		isConnectingNeboLoop = true;
		connectError = null;
		try {
			await neboLoopConnect({ code: connectCode.trim(), name: connectName.trim() });
			showConnectModal = false;
			await loadNeboLoopStatus();
		} catch (err: any) {
			connectError = err?.message || 'Connection failed';
		} finally {
			isConnectingNeboLoop = false;
		}
	}
</script>

<div class="max-w-5xl mx-auto px-4 py-6">
	<div class="mb-6 flex items-center justify-between">
		<div>
			<h1 class="font-display text-2xl font-bold text-base-content mb-1">App Store</h1>
			<p class="text-sm text-base-content/60">
				Browse and install apps and skills for your agent.
			</p>
		</div>
		<Button type="ghost" onclick={refreshStore}>
			<RefreshCw class="w-4 h-4 mr-2" />
			Refresh
		</Button>
	</div>

	<!-- NeboLoop connection status -->
	{#if neboLoopStatus}
		<Card>
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-3">
					{#if neboLoopStatus.connected}
						<Wifi class="w-5 h-5 text-success" />
						<div>
							<div class="text-sm font-medium text-base-content">
								Connected to NeboLoop
								{#if neboLoopStatus.botName}
									<span class="text-base-content/60">as {neboLoopStatus.botName}</span>
								{/if}
							</div>
							{#if neboLoopStatus.botId}
								<div class="text-xs text-base-content/40 font-mono">{neboLoopStatus.botId}</div>
							{/if}
						</div>
					{:else}
						<WifiOff class="w-5 h-5 text-base-content/40" />
						<div>
							<div class="text-sm font-medium text-base-content">Not connected to NeboLoop</div>
							<div class="text-xs text-base-content/40">Enter a connection code to browse the store</div>
						</div>
					{/if}
				</div>
				{#if !neboLoopStatus.connected}
					<button
						type="button"
						class="btn btn-sm btn-primary"
						onclick={openConnectModal}
					>
						<Link class="w-4 h-4 mr-1" />
						Connect
					</button>
				{/if}
			</div>
		</Card>
	{/if}

	<!-- Connect to NeboLoop modal -->
	{#if showConnectModal}
	<div class="modal modal-open">
		<div class="modal-box">
			<h3 class="text-lg font-bold">Connect to NeboLoop</h3>
			<p class="py-2 text-sm text-base-content/60">
				Enter your connection code from NeboLoop and a name for your bot.
			</p>
			<div class="space-y-4 mt-2">
				<div>
					<label for="connect-code" class="block text-sm font-medium text-base-content mb-1">Connection Code</label>
					<input
						id="connect-code"
						type="text"
						class="input input-bordered w-full font-mono"
						placeholder="NEBO-XXXX-XXXX"
						bind:value={connectCode}
						disabled={isConnectingNeboLoop}
					/>
				</div>
				<div>
					<label for="connect-name" class="block text-sm font-medium text-base-content mb-1">Bot Name</label>
					<input
						id="connect-name"
						type="text"
						class="input input-bordered w-full"
						placeholder="My Nebo Agent"
						bind:value={connectName}
						onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') handleConnect(); }}
						disabled={isConnectingNeboLoop}
					/>
				</div>
			</div>
			{#if connectError}
				<p class="text-error text-sm mt-2">{connectError}</p>
			{/if}
			<div class="modal-action">
				<button class="btn btn-ghost" onclick={() => showConnectModal = false} disabled={isConnectingNeboLoop}>Cancel</button>
				<button
					class="btn btn-primary"
					onclick={handleConnect}
					disabled={isConnectingNeboLoop || !connectCode.trim() || !connectName.trim()}
				>
					{#if isConnectingNeboLoop}
						<Loader2 class="w-4 h-4 animate-spin" />
						Connecting...
					{:else}
						Connect
					{/if}
				</button>
			</div>
		</div>
		<div class="modal-backdrop" onclick={() => showConnectModal = false}></div>
	</div>
	{/if}

	<!-- Apps / Skills sub-tabs -->
	<div class="flex gap-4 mb-4 mt-6">
		<button
			type="button"
			class="px-3 py-1.5 text-sm font-medium rounded-lg transition-colors
				{storeTab === 'apps'
					? 'bg-primary/10 text-primary'
					: 'text-base-content/60 hover:text-base-content hover:bg-base-200'}"
			onclick={() => switchStoreTab('apps')}
		>
			<Package class="w-4 h-4 inline-block mr-1 -mt-0.5" />
			Apps
		</button>
		<button
			type="button"
			class="px-3 py-1.5 text-sm font-medium rounded-lg transition-colors
				{storeTab === 'skills'
					? 'bg-primary/10 text-primary'
					: 'text-base-content/60 hover:text-base-content hover:bg-base-200'}"
			onclick={() => switchStoreTab('skills')}
		>
			<Zap class="w-4 h-4 inline-block mr-1 -mt-0.5" />
			Skills
		</button>
	</div>

	<!-- Search and filter bar -->
	<div class="flex gap-3 mb-6">
		<div class="flex-1 relative">
			<Search class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-base-content/40" />
			<input
				type="text"
				placeholder="Search {storeTab}..."
				class="w-full pl-10 pr-4 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50 text-sm"
				bind:value={storeSearch}
				onkeydown={(e) => e.key === 'Enter' && handleStoreSearch()}
			/>
		</div>
		<select
			class="px-3 py-2 rounded-lg bg-base-200 border border-base-300 focus:outline-none focus:ring-2 focus:ring-primary/50 text-sm"
			bind:value={storeCategory}
			onchange={handleStoreSearch}
		>
			<option value="">All Categories</option>
			<option value="communication">Communication</option>
			<option value="productivity">Productivity</option>
			<option value="development">Development</option>
			<option value="integration">Integrations</option>
			<option value="utility">Utilities</option>
		</select>
		<Button type="primary" size="sm" onclick={handleStoreSearch}>
			<Search class="w-4 h-4 mr-1" />
			Search
		</Button>
	</div>

	<!-- Apps listing -->
	{#if storeTab === 'apps'}
		{#if appsLoading}
			<Card>
				<div class="py-8 text-center text-base-content/60">Loading apps...</div>
			</Card>
		{:else if appsError}
			<Card>
				<div class="py-12 text-center">
					<Package class="w-12 h-12 mx-auto mb-3 text-base-content/30" />
					<p class="text-base-content/60 mb-2">Could not load apps</p>
					<p class="text-sm text-error mb-4">{appsError}</p>
					<p class="text-xs text-base-content/40">
						Make sure you are connected to NeboLoop above.
					</p>
				</div>
			</Card>
		{:else if storeApps.length === 0 && appsLoaded}
			<Card>
				<div class="py-12 text-center">
					<Package class="w-12 h-12 mx-auto mb-3 text-base-content/30" />
					<p class="text-base-content/60">No apps found.</p>
					{#if storeSearch || storeCategory}
						<button
							type="button"
							class="mt-3 text-sm text-primary hover:underline"
							onclick={() => { storeSearch = ''; storeCategory = ''; loadStoreApps(); }}
						>
							Clear filters
						</button>
					{/if}
				</div>
			</Card>
		{:else}
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
				{#each storeApps as app (app.id)}
					<Card>
						<div class="flex gap-3">
							<div class="w-12 h-12 rounded-xl bg-base-200 flex items-center justify-center text-xl shrink-0">
								{app.icon || '📦'}
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2 mb-1">
									<span class="font-semibold text-base-content truncate">{app.name}</span>
									{#if app.version}
										<span class="text-xs text-base-content/40">v{app.version}</span>
									{/if}
								</div>
								<p class="text-sm text-base-content/60 line-clamp-2 mb-2">{app.description}</p>
								<div class="flex items-center gap-3 text-xs text-base-content/50">
									{#if app.author}
										<span class="flex items-center gap-1">
											{app.author.name}
											{#if app.author.verified}
												<BadgeCheck class="w-3 h-3 text-primary" />
											{/if}
										</span>
									{/if}
									{#if app.category}
										<Badge variant="ghost" size="xs">{app.category}</Badge>
									{/if}
									{#if app.rating > 0}
										<span class="flex items-center gap-0.5">
											<Star class="w-3 h-3 text-warning" />
											{app.rating.toFixed(1)}
										</span>
									{/if}
									{#if app.installCount > 0}
										<span class="flex items-center gap-0.5">
											<Download class="w-3 h-3" />
											{app.installCount}
										</span>
									{/if}
								</div>
							</div>
						</div>
						<div class="mt-3 flex justify-end">
							{#if app.isInstalled}
								<Button
									type="ghost"
									size="xs"
									disabled={uninstalling[app.id]}
									onclick={() => handleUninstallApp(app)}
								>
									{#if uninstalling[app.id]}
										<Loader2 class="w-3 h-3 mr-1 animate-spin" />
									{:else}
										<Trash2 class="w-3 h-3 mr-1" />
									{/if}
									Uninstall
								</Button>
							{:else}
								<Button
									type="primary"
									size="xs"
									disabled={installing[app.id]}
									onclick={() => handleInstallApp(app)}
								>
									{#if installing[app.id]}
										<Loader2 class="w-3 h-3 mr-1 animate-spin" />
									{:else}
										<Download class="w-3 h-3 mr-1" />
									{/if}
									Install
								</Button>
							{/if}
						</div>
					</Card>
				{/each}
			</div>

			{#if appsTotalCount > appsPageSize}
				<div class="mt-6 flex items-center justify-center gap-2">
					<Button type="ghost" size="sm" disabled={appsPage <= 1} onclick={() => { appsPage--; loadStoreApps(); }}>
						Previous
					</Button>
					<span class="text-sm text-base-content/60">
						Page {appsPage} of {Math.ceil(appsTotalCount / appsPageSize)}
					</span>
					<Button type="ghost" size="sm" disabled={appsPage >= Math.ceil(appsTotalCount / appsPageSize)} onclick={() => { appsPage++; loadStoreApps(); }}>
						Next
					</Button>
				</div>
			{/if}
		{/if}
	{/if}

	<!-- Skills listing -->
	{#if storeTab === 'skills'}
		{#if skillsLoading}
			<Card>
				<div class="py-8 text-center text-base-content/60">Loading skills...</div>
			</Card>
		{:else if skillsError}
			<Card>
				<div class="py-12 text-center">
					<Zap class="w-12 h-12 mx-auto mb-3 text-base-content/30" />
					<p class="text-base-content/60 mb-2">Could not load skills</p>
					<p class="text-sm text-error mb-4">{skillsError}</p>
					<p class="text-xs text-base-content/40">
						Make sure you are connected to NeboLoop above.
					</p>
				</div>
			</Card>
		{:else if storeSkills.length === 0 && skillsLoaded}
			<Card>
				<div class="py-12 text-center">
					<Zap class="w-12 h-12 mx-auto mb-3 text-base-content/30" />
					<p class="text-base-content/60">No skills found.</p>
					{#if storeSearch || storeCategory}
						<button
							type="button"
							class="mt-3 text-sm text-primary hover:underline"
							onclick={() => { storeSearch = ''; storeCategory = ''; loadStoreSkills(); }}
						>
							Clear filters
						</button>
					{/if}
				</div>
			</Card>
		{:else}
			<div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
				{#each storeSkills as skill (skill.id)}
					<Card>
						<div class="flex gap-3">
							<div class="w-12 h-12 rounded-xl bg-base-200 flex items-center justify-center text-xl shrink-0">
								{skill.icon || '⚡'}
							</div>
							<div class="flex-1 min-w-0">
								<div class="flex items-center gap-2 mb-1">
									<span class="font-semibold text-base-content truncate">{skill.name}</span>
									{#if skill.version}
										<span class="text-xs text-base-content/40">v{skill.version}</span>
									{/if}
								</div>
								<p class="text-sm text-base-content/60 line-clamp-2 mb-2">{skill.description}</p>
								<div class="flex items-center gap-3 text-xs text-base-content/50">
									{#if skill.author}
										<span class="flex items-center gap-1">
											{skill.author.name}
											{#if skill.author.verified}
												<BadgeCheck class="w-3 h-3 text-primary" />
											{/if}
										</span>
									{/if}
									{#if skill.category}
										<Badge variant="ghost" size="xs">{skill.category}</Badge>
									{/if}
									{#if skill.rating > 0}
										<span class="flex items-center gap-0.5">
											<Star class="w-3 h-3 text-warning" />
											{skill.rating.toFixed(1)}
										</span>
									{/if}
									{#if skill.installCount > 0}
										<span class="flex items-center gap-0.5">
											<Download class="w-3 h-3" />
											{skill.installCount}
										</span>
									{/if}
								</div>
							</div>
						</div>
						<div class="mt-3 flex justify-end">
							{#if skill.isInstalled}
								<Button
									type="ghost"
									size="xs"
									disabled={uninstalling[skill.id]}
									onclick={() => handleUninstallSkill(skill)}
								>
									{#if uninstalling[skill.id]}
										<Loader2 class="w-3 h-3 mr-1 animate-spin" />
									{:else}
										<Trash2 class="w-3 h-3 mr-1" />
									{/if}
									Uninstall
								</Button>
							{:else}
								<Button
									type="primary"
									size="xs"
									disabled={installing[skill.id]}
									onclick={() => handleInstallSkill(skill)}
								>
									{#if installing[skill.id]}
										<Loader2 class="w-3 h-3 mr-1 animate-spin" />
									{:else}
										<Download class="w-3 h-3 mr-1" />
									{/if}
									Install
								</Button>
							{/if}
						</div>
					</Card>
				{/each}
			</div>

			{#if skillsTotalCount > skillsPageSize}
				<div class="mt-6 flex items-center justify-center gap-2">
					<Button type="ghost" size="sm" disabled={skillsPage <= 1} onclick={() => { skillsPage--; loadStoreSkills(); }}>
						Previous
					</Button>
					<span class="text-sm text-base-content/60">
						Page {skillsPage} of {Math.ceil(skillsTotalCount / skillsPageSize)}
					</span>
					<Button type="ghost" size="sm" disabled={skillsPage >= Math.ceil(skillsTotalCount / skillsPageSize)} onclick={() => { skillsPage++; loadStoreSkills(); }}>
						Next
					</Button>
				</div>
			{/if}
		{/if}
	{/if}
</div>
