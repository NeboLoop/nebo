<script lang="ts">
	import Modal from '$lib/components/ui/Modal.svelte';
	import Button from '$lib/components/ui/Button.svelte';
	import { Shield, Link2, Info, Unplug, ExternalLink, Star, Download, Globe, Lock, ChevronRight, Check, Clock, User, Layers, Monitor, Smartphone, ChevronDown, ChevronUp, MessageSquare, ThumbsUp } from 'lucide-svelte';
	import * as api from '$lib/api/nebo';
	import type { PluginItem, AppOAuthGrant, StoreApp, StoreAppDetail, StoreReview, GetStoreAppReviewsResponse } from '$lib/api/nebo';

	interface Props {
		plugin?: PluginItem | null;
		storeApp?: StoreApp | null;
		show: boolean;
		onclose: () => void;
		onupdated: () => void;
	}

	let { plugin = null, storeApp = null, show = $bindable(false), onclose, onupdated }: Props = $props();

	// Store detail data (loaded on open)
	let appDetail = $state<StoreAppDetail | null>(null);
	let reviews = $state<GetStoreAppReviewsResponse | null>(null);
	let loadingDetail = $state(false);
	let loadingReviews = $state(false);
	let installing = $state(false);

	// Installed app state
	let activeTab = $state<'settings' | 'connections' | 'info'>('settings');
	let savingSettings = $state(false);
	let settingsValues = $state<Record<string, string>>({});
	let oauthGrants = $state<AppOAuthGrant[]>([]);
	let loadingGrants = $state(false);
	let disconnectingProvider = $state<string | null>(null);

	// UI state
	let descriptionExpanded = $state(false);
	let showAllReviews = $state(false);

	// Derived
	const isStoreApp = $derived(!plugin && !!storeApp);
	const appName = $derived(plugin?.displayName || plugin?.name || storeApp?.name || 'App');
	const appIcon = $derived(plugin?.icon || storeApp?.icon || '');
	const appDescription = $derived(appDetail?.description || storeApp?.description || plugin?.description || '');
	const appVersion = $derived(appDetail?.version || storeApp?.version || plugin?.version || '');
	const appCategory = $derived(appDetail?.category || storeApp?.category || '');
	const appAuthor = $derived(storeApp?.author || appDetail?.author);
	const appRating = $derived(appDetail?.rating ?? storeApp?.rating ?? 0);
	const appReviewCount = $derived(appDetail?.reviewCount ?? storeApp?.reviewCount ?? 0);
	const appInstallCount = $derived(appDetail?.installCount ?? storeApp?.installCount ?? 0);
	const isInstalled = $derived(plugin?.isInstalled || storeApp?.isInstalled || appDetail?.isInstalled || false);

	// Load data when modal opens
	$effect(() => {
		if (show && storeApp) {
			loadStoreDetail(storeApp.id);
			loadReviews(storeApp.id);
		}
		if (show && plugin) {
			settingsValues = plugin.settings ? { ...plugin.settings } : {};
			// Auto-open Settings tab if app needs setup, otherwise show Info
			activeTab = pluginNeedsSetup() ? 'settings' : 'info';
			loadOAuthGrants();
		}
		if (!show) {
			appDetail = null;
			reviews = null;
			descriptionExpanded = false;
			showAllReviews = false;
		}
	});

	async function loadStoreDetail(id: string) {
		loadingDetail = true;
		try {
			const resp = await api.getStoreApp(id);
			appDetail = resp.app;
		} catch (error) {
			console.error('Failed to load app detail:', error);
		} finally {
			loadingDetail = false;
		}
	}

	async function loadReviews(id: string) {
		loadingReviews = true;
		try {
			reviews = await api.getStoreAppReviews(id);
		} catch {
			reviews = null;
		} finally {
			loadingReviews = false;
		}
	}

	async function handleInstall() {
		const id = storeApp?.id || appDetail?.id;
		if (!id) return;
		installing = true;
		try {
			await api.installStoreApp(id);
			if (appDetail) appDetail = { ...appDetail, isInstalled: true };
			if (storeApp) storeApp = { ...storeApp, isInstalled: true };
			onupdated();
		} catch (error) {
			console.error('Failed to install:', error);
		} finally {
			installing = false;
		}
	}

	async function handleUninstall() {
		const id = storeApp?.id || appDetail?.id;
		if (!id) return;
		installing = true;
		try {
			await api.uninstallStoreApp(id);
			if (appDetail) appDetail = { ...appDetail, isInstalled: false };
			if (storeApp) storeApp = { ...storeApp, isInstalled: false };
			onupdated();
		} catch (error) {
			console.error('Failed to uninstall:', error);
		} finally {
			installing = false;
		}
	}

	// --- Settings (for installed apps) ---

	interface SettingsField {
		key: string;
		title: string;
		type: string;
		description?: string;
		required?: boolean;
		secret?: boolean;
		placeholder?: string;
		options?: Array<{ label: string; value: string }>;
		default?: string;
	}

	interface SettingsGroup {
		title: string;
		description?: string;
		fields: SettingsField[];
	}

	function parseSettingsGroups(manifest: any): SettingsGroup[] {
		if (!manifest) return [];
		if (manifest.groups && Array.isArray(manifest.groups)) return manifest.groups;
		return [];
	}

	function hasSettings(manifest: any): boolean {
		return parseSettingsGroups(manifest).some(g => g.fields?.length > 0);
	}

	async function saveSettings() {
		if (!plugin) return;
		savingSettings = true;
		try {
			const secrets: Record<string, boolean> = {};
			for (const group of parseSettingsGroups(plugin.settingsManifest)) {
				for (const field of group.fields) {
					if (field.secret || field.type === 'password') {
						secrets[field.key] = true;
					}
				}
			}
			await api.updatePluginSettings({ settings: settingsValues, secrets }, plugin.id);
			onupdated();
		} catch (error) {
			console.error('Failed to save settings:', error);
		} finally {
			savingSettings = false;
		}
	}

	// --- OAuth ---

	async function loadOAuthGrants() {
		if (!plugin) return;
		loadingGrants = true;
		try {
			const resp = await api.getAppOAuthGrants(plugin.id);
			oauthGrants = resp.grants || [];
		} catch {
			oauthGrants = [];
		} finally {
			loadingGrants = false;
		}
	}

	function connectOAuth(provider: string) {
		if (!plugin) return;
		const url = api.getAppOAuthConnectUrl(plugin.id, provider);
		window.open(url, '_blank', 'width=600,height=700');
	}

	async function disconnectOAuth(provider: string) {
		if (!plugin) return;
		disconnectingProvider = provider;
		try {
			await api.disconnectAppOAuth(plugin.id, provider);
			await loadOAuthGrants();
		} catch (error) {
			console.error('Failed to disconnect OAuth:', error);
		} finally {
			disconnectingProvider = null;
		}
	}

	function handleClose() {
		show = false;
		onclose();
	}

	function formatBytes(bytes: number): string {
		if (bytes < 1024) return bytes + ' B';
		if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB';
		return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
	}

	function formatCount(count: number): string {
		if (count >= 1000000) return (count / 1000000).toFixed(1) + 'M';
		if (count >= 1000) return (count / 1000).toFixed(1) + 'K';
		return count.toString();
	}

	function renderStars(rating: number): string {
		const full = Math.floor(rating);
		const half = rating - full >= 0.5 ? 1 : 0;
		const empty = 5 - full - half;
		return '★'.repeat(full) + (half ? '½' : '') + '☆'.repeat(empty);
	}

	const groups = $derived(plugin ? parseSettingsGroups(plugin.settingsManifest) : []);
	const showSettings = $derived(groups.some(g => g.fields?.length > 0));
	const pluginNeedsSetup = $derived(() => {
		if (!plugin || !showSettings) return false;
		const required = groups.flatMap(g => g.fields).filter(f => f.required).map(f => f.key);
		if (required.length === 0) return false;
		const settings = plugin.settings || {};
		return required.some(key => !settings[key] || settings[key] === '••••••••');
	});
	const latestChangelog = $derived(appDetail?.changelog?.[0] || null);
	const displayedReviews = $derived(
		showAllReviews
			? (reviews?.reviews || [])
			: (reviews?.reviews || []).slice(0, 3)
	);
	const totalSize = $derived(() => {
		if (!appDetail?.size) return null;
		const values = Object.values(appDetail.size);
		if (values.length === 0) return null;
		return values[0];
	});
</script>

<Modal bind:show title="" size="full" onclose={handleClose} closeOnBackdrop={true} showCloseButton={false}>
	<!-- Custom header with close button -->
	<div class="flex items-center justify-between -mt-4 pb-4 border-b border-base-300 mb-6">
		<button onclick={handleClose} class="btn btn-sm btn-ghost">
			← Back
		</button>
		<div class="flex items-center gap-2">
			{#if isStoreApp || storeApp}
				{#if isInstalled}
					<button
						class="btn btn-sm btn-ghost text-success"
						onclick={handleUninstall}
						disabled={installing}
					>
						{#if installing}
							<span class="loading loading-spinner loading-xs"></span>
						{:else}
							<Check class="w-4 h-4" />
							Installed
						{/if}
					</button>
				{:else}
					<button
						class="btn btn-sm btn-primary"
						onclick={handleInstall}
						disabled={installing}
					>
						{#if installing}
							<span class="loading loading-spinner loading-xs"></span>
						{:else}
							Get
						{/if}
					</button>
				{/if}
			{/if}
		</div>
	</div>

	{#if loadingDetail && !appDetail && !plugin}
		<div class="py-20 text-center text-base-content/60">
			<span class="loading loading-spinner loading-lg"></span>
			<p class="mt-4">Loading app details...</p>
		</div>
	{:else}
		<!-- Hero Section -->
		<div class="flex items-start gap-5 mb-8">
			<div class="w-20 h-20 rounded-2xl bg-base-200 flex items-center justify-center shrink-0 overflow-hidden">
				{#if appIcon}
					<img src={appIcon} alt={appName} class="w-full h-full object-cover rounded-2xl" />
				{:else}
					<span class="text-3xl font-bold text-primary">{(appName).charAt(0).toUpperCase()}</span>
				{/if}
			</div>
			<div class="flex-1 min-w-0">
				<h2 class="text-2xl font-bold text-base-content mb-1">{appName}</h2>
				{#if appAuthor}
					<p class="text-sm text-primary mb-1">
						{appAuthor.name}
						{#if appAuthor.verified}
							<Check class="w-3.5 h-3.5 inline text-primary" />
						{/if}
					</p>
				{/if}
				{#if appCategory}
					<p class="text-sm text-base-content/50">{appCategory}</p>
				{/if}
				{#if plugin}
					<div class="flex items-center gap-2 mt-2">
						<span class="badge badge-sm badge-outline">v{plugin.version}</span>
						{#if pluginNeedsSetup()}
							<span class="badge badge-sm badge-warning">Needs Setup</span>
						{:else}
							<span class="badge badge-sm {plugin.connectionStatus === 'connected' ? 'badge-success' : plugin.connectionStatus === 'error' ? 'badge-error' : 'badge-ghost'}">
								{plugin.connectionStatus}
							</span>
						{/if}
					</div>
				{/if}
			</div>
		</div>

		<!-- Metadata Chips Row -->
		{#if isStoreApp || storeApp}
			<div class="flex items-center gap-0 overflow-x-auto mb-8">
				{#if appRating > 0}
					<div class="flex flex-col items-center px-4 min-w-[5rem] border-r border-base-300">
						<span class="text-xs text-base-content/40 uppercase mb-1">{appReviewCount} Ratings</span>
						<span class="text-xl font-bold text-base-content">{appRating.toFixed(1)}</span>
						<span class="text-xs text-warning">{renderStars(appRating)}</span>
					</div>
				{/if}
				{#if appDetail?.ageRating}
					<div class="flex flex-col items-center px-4 min-w-[5rem] border-r border-base-300">
						<span class="text-xs text-base-content/40 uppercase mb-1">Age</span>
						<span class="text-xl font-bold text-base-content">{appDetail.ageRating}</span>
						<span class="text-xs text-base-content/40">Years Old</span>
					</div>
				{/if}
				{#if appCategory}
					<div class="flex flex-col items-center px-4 min-w-[5rem] border-r border-base-300">
						<span class="text-xs text-base-content/40 uppercase mb-1">Category</span>
						<Layers class="w-5 h-5 text-base-content mb-0.5" />
						<span class="text-xs text-base-content/60">{appCategory}</span>
					</div>
				{/if}
				{#if totalSize()}
					<div class="flex flex-col items-center px-4 min-w-[5rem] border-r border-base-300">
						<span class="text-xs text-base-content/40 uppercase mb-1">Size</span>
						<span class="text-xl font-bold text-base-content">{formatBytes(totalSize()!)}</span>
					</div>
				{/if}
				{#if appDetail?.language}
					<div class="flex flex-col items-center px-4 min-w-[5rem]">
						<span class="text-xs text-base-content/40 uppercase mb-1">Language</span>
						<span class="text-xl font-bold text-base-content">{appDetail.language.toUpperCase().slice(0, 2)}</span>
						<span class="text-xs text-base-content/40">{appDetail.language}</span>
					</div>
				{/if}
				{#if appInstallCount > 0}
					<div class="flex flex-col items-center px-4 min-w-[5rem] border-l border-base-300">
						<span class="text-xs text-base-content/40 uppercase mb-1">Downloads</span>
						<span class="text-xl font-bold text-base-content">{formatCount(appInstallCount)}</span>
					</div>
				{/if}
			</div>
		{/if}

		<!-- Screenshots -->
		{#if appDetail?.screenshots && appDetail.screenshots.length > 0}
			<div class="mb-8">
				<div class="flex gap-3 overflow-x-auto pb-2">
					{#each appDetail.screenshots as screenshot}
						<img
							src={screenshot}
							alt="Screenshot"
							class="h-48 rounded-lg object-cover shrink-0 bg-base-200"
						/>
					{/each}
				</div>
			</div>
		{/if}

		<!-- Description -->
		{#if appDescription}
			<div class="mb-8">
				<div class="relative">
					<p class="text-sm text-base-content leading-relaxed {descriptionExpanded ? '' : 'line-clamp-3'}">
						{appDescription}
					</p>
					{#if appDescription.length > 200}
						<button
							class="text-sm text-primary font-medium mt-1 flex items-center gap-1"
							onclick={() => descriptionExpanded = !descriptionExpanded}
						>
							{descriptionExpanded ? 'Less' : 'More'}
							{#if descriptionExpanded}
								<ChevronUp class="w-4 h-4" />
							{:else}
								<ChevronDown class="w-4 h-4" />
							{/if}
						</button>
					{/if}
				</div>
			</div>
		{/if}

		<!-- What's New -->
		{#if latestChangelog}
			<div class="mb-8">
				<h3 class="text-lg font-bold text-base-content mb-3">What's New</h3>
				<div class="flex items-center gap-2 mb-2">
					<span class="text-sm text-base-content/60">Version {latestChangelog.version}</span>
					{#if latestChangelog.date}
						<span class="text-sm text-base-content/40">&middot; {latestChangelog.date}</span>
					{/if}
				</div>
				<p class="text-sm text-base-content/80 whitespace-pre-line">{latestChangelog.notes}</p>
			</div>
		{/if}

		<!-- Ratings & Reviews -->
		{#if (isStoreApp || storeApp) && (appRating > 0 || reviews)}
			<div class="mb-8">
				<h3 class="text-lg font-bold text-base-content mb-3">Ratings & Reviews</h3>

				{#if reviews}
					<div class="flex gap-8 mb-6">
						<!-- Big rating number -->
						<div class="text-center shrink-0">
							<div class="text-5xl font-bold text-base-content">{reviews.average.toFixed(1)}</div>
							<div class="text-xs text-warning mt-1">{renderStars(reviews.average)}</div>
							<div class="text-xs text-base-content/40 mt-1">{formatCount(reviews.totalCount)} Ratings</div>
						</div>

						<!-- Distribution bars -->
						<div class="flex-1 flex flex-col justify-center gap-1">
							{#each [5, 4, 3, 2, 1] as starCount}
								{@const count = reviews.distribution[starCount - 1] || 0}
								{@const maxCount = Math.max(...reviews.distribution, 1)}
								<div class="flex items-center gap-2">
									<span class="text-xs text-base-content/50 w-3 text-right">{starCount}</span>
									<Star class="w-3 h-3 text-warning shrink-0" />
									<div class="flex-1 bg-base-200 rounded-full h-2 overflow-hidden">
										<div
											class="bg-warning h-full rounded-full transition-all"
											style="width: {(count / maxCount) * 100}%"
										></div>
									</div>
								</div>
							{/each}
						</div>
					</div>

					<!-- Review cards -->
					{#if displayedReviews.length > 0}
						<div class="grid sm:grid-cols-2 gap-3">
							{#each displayedReviews as review}
								<div class="rounded-xl bg-base-200/50 p-4">
									<div class="flex items-center justify-between mb-2">
										<div class="flex items-center gap-2">
											<div class="w-7 h-7 rounded-full bg-base-300 flex items-center justify-center">
												<User class="w-4 h-4 text-base-content/40" />
											</div>
											<span class="text-sm font-medium text-base-content">{review.userName}</span>
										</div>
										<span class="text-xs text-base-content/40">{review.createdAt}</span>
									</div>
									<div class="text-xs text-warning mb-2">{renderStars(review.rating)}</div>
									{#if review.title}
										<p class="text-sm font-semibold text-base-content mb-1">{review.title}</p>
									{/if}
									<p class="text-sm text-base-content/70 line-clamp-3">{review.body}</p>
									{#if review.helpful > 0}
										<div class="flex items-center gap-1 mt-2 text-xs text-base-content/40">
											<ThumbsUp class="w-3 h-3" />
											{review.helpful} found helpful
										</div>
									{/if}
								</div>
							{/each}
						</div>
						{#if (reviews.reviews?.length || 0) > 3}
							<button
								class="btn btn-ghost btn-sm mt-3"
								onclick={() => showAllReviews = !showAllReviews}
							>
								{showAllReviews ? 'Show Less' : `See All ${reviews.totalCount} Reviews`}
								<ChevronRight class="w-4 h-4" />
							</button>
						{/if}
					{/if}
				{:else if loadingReviews}
					<div class="py-4 text-center text-base-content/40">
						<span class="loading loading-spinner loading-sm"></span>
					</div>
				{/if}
			</div>
		{/if}

		<!-- Information Grid -->
		{#if isStoreApp || storeApp}
			<div class="mb-8">
				<h3 class="text-lg font-bold text-base-content mb-3">Information</h3>
				<div class="grid grid-cols-2 sm:grid-cols-3 gap-4 text-sm">
					{#if appAuthor}
						<div>
							<span class="text-base-content/40 block mb-0.5">Provider</span>
							<p class="font-medium text-base-content">{appAuthor.name}</p>
						</div>
					{/if}
					{#if totalSize()}
						<div>
							<span class="text-base-content/40 block mb-0.5">Size</span>
							<p class="font-medium text-base-content">{formatBytes(totalSize()!)}</p>
						</div>
					{/if}
					{#if appCategory}
						<div>
							<span class="text-base-content/40 block mb-0.5">Category</span>
							<p class="font-medium text-base-content">{appCategory}</p>
						</div>
					{/if}
					{#if appDetail?.platforms && appDetail.platforms.length > 0}
						<div>
							<span class="text-base-content/40 block mb-0.5">Platforms</span>
							<p class="font-medium text-base-content">{appDetail.platforms.join(', ')}</p>
						</div>
					{/if}
					{#if appDetail?.language}
						<div>
							<span class="text-base-content/40 block mb-0.5">Language</span>
							<p class="font-medium text-base-content">{appDetail.language}</p>
						</div>
					{/if}
					{#if appDetail?.ageRating}
						<div>
							<span class="text-base-content/40 block mb-0.5">Age Rating</span>
							<p class="font-medium text-base-content">{appDetail.ageRating}</p>
						</div>
					{/if}
					<div>
						<span class="text-base-content/40 block mb-0.5">Version</span>
						<p class="font-medium text-base-content">{appVersion}</p>
					</div>
				</div>

				<!-- Links -->
				{#if appDetail?.websiteUrl || appDetail?.privacyUrl || appDetail?.supportUrl}
					<div class="flex gap-4 mt-4">
						{#if appDetail?.websiteUrl}
							<a href={appDetail.websiteUrl} target="_blank" rel="noopener noreferrer" class="text-sm text-primary flex items-center gap-1">
								<Globe class="w-3.5 h-3.5" /> Website
							</a>
						{/if}
						{#if appDetail?.privacyUrl}
							<a href={appDetail.privacyUrl} target="_blank" rel="noopener noreferrer" class="text-sm text-primary flex items-center gap-1">
								<Lock class="w-3.5 h-3.5" /> Privacy Policy
							</a>
						{/if}
						{#if appDetail?.supportUrl}
							<a href={appDetail.supportUrl} target="_blank" rel="noopener noreferrer" class="text-sm text-primary flex items-center gap-1">
								<MessageSquare class="w-3.5 h-3.5" /> Support
							</a>
						{/if}
					</div>
				{/if}
			</div>
		{/if}

		<!-- Installed App: Settings / Connections / Info Tabs -->
		{#if plugin}
			<div class="border-t border-base-300 pt-6">
				<!-- Tabs -->
				<div role="tablist" class="tabs tabs-bordered mb-4">
					<button
						role="tab"
						class="tab"
						class:tab-active={activeTab === 'info'}
						onclick={() => activeTab = 'info'}
					>
						Info
					</button>
					{#if showSettings}
						<button
							role="tab"
							class="tab"
							class:tab-active={activeTab === 'settings'}
							onclick={() => activeTab = 'settings'}
						>
							Settings
						</button>
					{/if}
					<button
						role="tab"
						class="tab"
						class:tab-active={activeTab === 'connections'}
						onclick={() => activeTab = 'connections'}
					>
						Connections
					</button>
				</div>

				<!-- Tab content -->
				{#if activeTab === 'settings' && showSettings}
					<div class="space-y-4">
						{#each groups as group}
							{#if group.title}
								<h4 class="font-medium text-sm text-base-content">{group.title}</h4>
							{/if}
							{#if group.description}
								<p class="text-xs text-base-content/50">{group.description}</p>
							{/if}
							{#each group.fields as field}
								<div class="form-control">
									<label class="label" for="modal-setting-{field.key}">
										<span class="label-text font-medium">{field.title}</span>
										{#if field.required}
											<span class="label-text-alt text-error">Required</span>
										{/if}
									</label>
									{#if field.description}
										<p class="text-xs text-base-content/50 mb-1">{field.description}</p>
									{/if}
									{#if field.type === 'toggle'}
										<input
											id="modal-setting-{field.key}"
											type="checkbox"
											class="toggle toggle-primary toggle-sm"
											checked={settingsValues[field.key] === 'true'}
											onchange={(e) => {
												settingsValues[field.key] = (e.target as HTMLInputElement).checked ? 'true' : 'false';
											}}
										/>
									{:else if field.type === 'select' && field.options}
										<select
											id="modal-setting-{field.key}"
											class="select select-bordered select-sm w-full"
											value={settingsValues[field.key] || field.default || ''}
											onchange={(e) => {
												settingsValues[field.key] = (e.target as HTMLSelectElement).value;
											}}
										>
											{#each field.options as opt}
												<option value={opt.value}>{opt.label}</option>
											{/each}
										</select>
									{:else}
										<input
											id="modal-setting-{field.key}"
											type={field.type === 'password' || field.secret ? 'password' : field.type === 'number' ? 'number' : field.type === 'url' ? 'url' : 'text'}
											class="input input-bordered input-sm w-full"
											placeholder={field.placeholder || ''}
											value={settingsValues[field.key] || ''}
											oninput={(e) => {
												settingsValues[field.key] = (e.target as HTMLInputElement).value;
											}}
										/>
									{/if}
								</div>
							{/each}
						{/each}
						<div class="flex justify-end pt-2">
							<Button type="primary" size="sm" onclick={saveSettings} disabled={savingSettings}>
								{#if savingSettings}
									<span class="loading loading-spinner loading-xs"></span>
								{/if}
								Save Settings
							</Button>
						</div>
					</div>
				{:else if activeTab === 'connections'}
					<div class="space-y-3">
						{#if loadingGrants}
							<div class="py-6 text-center text-base-content/60">
								<span class="loading loading-spinner loading-sm"></span>
								<p class="mt-2 text-sm">Loading connections...</p>
							</div>
						{:else if oauthGrants.length > 0}
							{#each oauthGrants as grant}
								<div class="flex items-center justify-between p-3 rounded-lg bg-base-200/50">
									<div class="flex items-center gap-3">
										<Link2 class="w-4 h-4 text-base-content/60" />
										<div>
											<p class="font-medium text-sm capitalize">{grant.provider}</p>
											<p class="text-xs text-base-content/50">
												{grant.connection_status === 'connected' ? 'Connected' : grant.connection_status}
												{#if grant.scopes}
													 &middot; {grant.scopes}
												{/if}
											</p>
										</div>
									</div>
									<div class="flex items-center gap-2">
										{#if grant.connection_status === 'connected'}
											<button
												class="btn btn-xs btn-ghost text-error"
												onclick={() => disconnectOAuth(grant.provider)}
												disabled={disconnectingProvider === grant.provider}
											>
												{#if disconnectingProvider === grant.provider}
													<span class="loading loading-spinner loading-xs"></span>
												{:else}
													<Unplug class="w-3 h-3" />
													Disconnect
												{/if}
											</button>
										{:else}
											<button
												class="btn btn-xs btn-primary"
												onclick={() => connectOAuth(grant.provider)}
											>
												<ExternalLink class="w-3 h-3" />
												Connect
											</button>
										{/if}
									</div>
								</div>
							{/each}
						{:else}
							<div class="py-6 text-center text-base-content/60">
								<Link2 class="w-8 h-8 mx-auto mb-2 opacity-20" />
								<p class="text-sm">No OAuth connections configured for this app.</p>
							</div>
						{/if}
					</div>
				{:else if activeTab === 'info'}
					<div class="space-y-4">
						{#if plugin.description}
							<div>
								<h4 class="text-sm font-medium text-base-content/60 mb-1">Description</h4>
								<p class="text-sm text-base-content">{plugin.description}</p>
							</div>
						{/if}

						{#if plugin.capabilities && plugin.capabilities.length > 0}
							<div>
								<h4 class="text-sm font-medium text-base-content/60 mb-2">Capabilities</h4>
								<div class="flex flex-wrap gap-2">
									{#each plugin.capabilities as cap}
										<span class="badge badge-sm badge-primary badge-outline">{cap}</span>
									{/each}
								</div>
							</div>
						{/if}

						{#if plugin.permissions && plugin.permissions.length > 0}
							<div>
								<h4 class="text-sm font-medium text-base-content/60 mb-2">Permissions</h4>
								<div class="flex flex-wrap gap-2">
									{#each plugin.permissions as perm}
										<span class="badge badge-sm badge-warning badge-outline">
											<Shield class="w-3 h-3 mr-1" />
											{perm}
										</span>
									{/each}
								</div>
							</div>
						{/if}

						<div class="grid grid-cols-2 gap-3 text-sm">
							<div>
								<span class="text-base-content/50">Type</span>
								<p class="font-medium">{plugin.pluginType}</p>
							</div>
							<div>
								<span class="text-base-content/50">Version</span>
								<p class="font-medium">{plugin.version}</p>
							</div>
							<div>
								<span class="text-base-content/50">Installed</span>
								<p class="font-medium">{plugin.createdAt}</p>
							</div>
							{#if plugin.lastConnectedAt}
								<div>
									<span class="text-base-content/50">Last Connected</span>
									<p class="font-medium">{plugin.lastConnectedAt}</p>
								</div>
							{/if}
						</div>

						{#if plugin.lastError}
							<div class="alert alert-error alert-sm">
								<Info class="w-4 h-4" />
								<span class="text-sm">{plugin.lastError}</span>
							</div>
						{/if}
					</div>
				{/if}
			</div>
		{/if}
	{/if}
</Modal>
