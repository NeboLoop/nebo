<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { beforeNavigate } from '$app/navigation';
	import { AppNav, SideNav } from '$lib/components/navigation';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { auth } from '$lib/stores/auth';
	import { get } from 'svelte/store';
	import { settingsReturnPath } from '$lib/stores/settings';
	import OnboardingFlow from '$lib/components/onboarding/OnboardingFlow.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';

	import {
		checkForUpdate,
		resetUpdateState,
		updateInfo,
		downloadProgress,
		updateReady,
		updateError,
		autoUpdateEnabled,
		type DownloadProgress
	} from '$lib/stores/update';
	import type { UpdateCheckResponse } from '$lib/api/neboComponents';
	import * as api from '$lib/api/nebo';
	import CommandPalette from '$lib/components/CommandPalette.svelte';
	import WhatsNewModal from '$lib/components/WhatsNewModal.svelte';
	import Toast from '$lib/components/ui/Toast.svelte';

	let { children }: { children: Snippet } = $props();

	let commandPaletteOpen = $state(false);

	// What's New modal state
	let showWhatsNew = $state(false);
	let whatsNewVersion = $state('');
	let whatsNewReleaseUrl = $state<string | undefined>(undefined);

	// Toast state for tray "Check for Updates" when already up-to-date
	let showUpToDateToast = $state(false);

	// Theme: detect OS preference, allow user override
	let themePref = $state<'light' | 'dark' | 'system'>('system');
	let systemDark = $state(true);

	const resolvedTheme = $derived(
		themePref === 'system' ? (systemDark ? 'dark' : 'light') : themePref
	);

	// Onboarding state
	let isCheckingOnboarding = $state(true);
	let showOnboarding = $state(false);

	// Quarantine notices from NeboLoop
	interface QuarantineNotice {
		app_id: string;
		app_name: string;
		reason: string;
	}
	let quarantineNotices = $state<QuarantineNotice[]>([]);

	// Full-height pages that bypass the normal padded layout (chat only)
	const isFullHeightRoute = $derived(
		$page.url.pathname.startsWith('/agent')
	);

	// Edge-to-edge routes with their own sidebar (no centering wrapper)
	const isEdgeToEdgeRoute = $derived(
		$page.url.pathname.startsWith('/marketplace')
	);

	// Settings renders as a centered modal overlay
	const isSettingsRoute = $derived(
		$page.url.pathname.startsWith('/settings')
	);

	// Upgrade funnel renders full-screen, no sidebar
	const isUpgradeRoute = $derived(
		$page.url.pathname.startsWith('/upgrade')
	);

	// Capture return path when navigating into settings
	// Capture return path when entering settings — but never return to transient routes
	const transientRoutes = ['/upgrade', '/settings'];
	beforeNavigate(({ from, to }) => {
		if (to?.url.pathname.startsWith('/settings') && !from?.url.pathname.startsWith('/settings')) {
			const fromPath = from?.url.pathname ?? '/';
			if (!transientRoutes.some(r => fromPath.startsWith(r))) {
				settingsReturnPath.set(fromPath);
			}
		}
	});

	onMount(async () => {
		// Theme: listen to OS preference changes in real time
		const mq = window.matchMedia('(prefers-color-scheme: dark)');
		systemDark = mq.matches;
		const onSystemChange = (e: MediaQueryListEvent) => { systemDark = e.matches; };
		mq.addEventListener('change', onSystemChange);

		// Sync resolved theme to <html data-theme="...">
		$effect(() => { document.documentElement.setAttribute('data-theme', resolvedTheme); });

		// Connect WebSocket — pass token from auth store (in-memory)
		const wsClient = getWebSocketClient();
		const authState = get(auth);
		wsClient.connect(authState.token ?? undefined);

		// Listen for quarantine events from the agent
		const unsubQuarantine = wsClient.on<QuarantineNotice>('app_quarantined', (data) => {
			if (data) {
				quarantineNotices = [...quarantineNotices, data];
			}
		});

		// Listen for background update_available events pushed by the server
		const unsubUpdate = wsClient.on<UpdateCheckResponse & { autoUpdateEnabled?: boolean }>('update_available', (data) => {
			if (data) {
				updateError.set(null);
				updateInfo.set({ ...data, available: true });
				autoUpdateEnabled.set(data.autoUpdateEnabled ?? true);
			}
		});

		// Listen for download progress events
		const unsubProgress = wsClient.on<DownloadProgress>('update_progress', (data) => {
			if (data) {
				downloadProgress.set(data);
			}
		});

		// Listen for update ready events (download complete, verified)
		const unsubReady = wsClient.on<{ version: string }>('update_ready', (data) => {
			if (data) {
				downloadProgress.set(null);
				updateReady.set(data.version);
			}
		});

		// Listen for update error events
		const unsubError = wsClient.on<{ error: string }>('update_error', (data) => {
			if (data) {
				downloadProgress.set(null);
				updateError.set(data.error);
			}
		});

		// Listen for plan change events (NeboLoop billing upgrade)
		const unsubPlan = wsClient.on<{ plan: string }>('plan_changed', (data) => {
			if (data) {
				window.dispatchEvent(new CustomEvent('nebo:plan_changed', { detail: data }));
			}
		});

		// On connect (initial + reconnect after restart): reset stale update
		// state and re-check. onStatus fires immediately with current status,
		// so this also covers the initial check.
		const unsubStatus = wsClient.onStatus((status) => {
			if (status === 'connected') {
				resetUpdateState();
				checkForUpdate();
			}
		});

		// What's New: detect version change after update
		const unsubWhatsNew = updateInfo.subscribe((info) => {
			if (!info?.currentVersion) return;
			const lastSeen = localStorage.getItem('nebo_last_seen_version');
			if (lastSeen === null) {
				// First install — seed silently, no modal
				localStorage.setItem('nebo_last_seen_version', info.currentVersion);
			} else if (lastSeen !== info.currentVersion) {
				whatsNewVersion = info.currentVersion;
				whatsNewReleaseUrl = info.releaseUrl;
				showWhatsNew = true;
			}
		});

		// Tray menu: "Check for Updates" handler
		(window as any).__NEBO_CHECK_UPDATE__ = async () => {
			resetUpdateState();
			await checkForUpdate();
			// If no update available, show "up to date" toast
			const info = get(updateInfo);
			if (!info?.available) {
				showUpToDateToast = true;
			}
		};

		// Check if user needs onboarding + load theme preference
		try {
			const [response, prefsData] = await Promise.all([
				api.getUserProfile(),
				api.getPreferences().catch(() => ({ preferences: null }))
			]);
			showOnboarding = !response.profile?.onboardingCompleted;
			const savedTheme = prefsData.preferences?.theme;
			if (savedTheme === 'light' || savedTheme === 'dark' || savedTheme === 'system') {
				themePref = savedTheme;
			}
		} catch (err) {
			// If we can't get profile, show onboarding
			showOnboarding = true;
		} finally {
			isCheckingOnboarding = false;
		}

		return () => {
			mq.removeEventListener('change', onSystemChange);
			unsubQuarantine();
			unsubUpdate();
			unsubProgress();
			unsubReady();
			unsubError();
			unsubPlan();
			unsubStatus();
			unsubWhatsNew();
			delete (window as any).__NEBO_CHECK_UPDATE__;
		};
	});

	function dismissQuarantine(appId: string) {
		quarantineNotices = quarantineNotices.filter(n => n.app_id !== appId);
	}
</script>

<svelte:window onkeydown={(e) => {
	if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
		e.preventDefault();
		commandPaletteOpen = !commandPaletteOpen;
	}
}} />

<!-- Show onboarding if needed -->
{#if showOnboarding && !isCheckingOnboarding}
	<OnboardingFlow onComplete={() => { showOnboarding = false; }} />
{:else if isCheckingOnboarding}
	<!-- Loading state while checking onboarding -->
	<div class="h-dvh flex items-center justify-center bg-base-100">
		<div class="loading loading-spinner loading-lg"></div>
	</div>
{:else}
	<div class="h-dvh flex flex-col overflow-hidden bg-base-100">
		<AppNav />
		{#each quarantineNotices as notice (notice.app_id)}
			<div class="px-4 pt-2">
				<Alert type="warning" title="{notice.app_name} was removed due to a security concern." dismissible onclose={() => dismissQuarantine(notice.app_id)}>
					Your data is safe. This app was automatically stopped and quarantined by NeboLoop.
				</Alert>
			</div>
		{/each}
		<div class="flex flex-1 min-h-0 overflow-hidden">
			<SideNav />
			{#if isUpgradeRoute}
				<main id="main-content" class="flex-1 min-w-0 overflow-y-auto">
					{@render children()}
				</main>
			{:else if isFullHeightRoute}
				<main id="main-content" class="flex-1 flex flex-col min-h-0 min-w-0 overflow-hidden">
					{@render children()}
				</main>
			{:else if isSettingsRoute}
				<div class="flex-1 min-w-0 overflow-hidden">
					{@render children()}
				</div>
			{:else if isEdgeToEdgeRoute}
				<main id="main-content" class="flex-1 min-w-0 px-6 pt-6 overflow-y-auto">
					{@render children()}
				</main>
			{:else}
				<main id="main-content" class="flex-1 min-w-0 p-6 overflow-y-auto">
					<div class="max-w-[1400px] mx-auto">
						{@render children()}
					</div>
				</main>
			{/if}
		</div>
	</div>
{/if}

<CommandPalette bind:open={commandPaletteOpen} onclose={() => commandPaletteOpen = false} />

<WhatsNewModal
	bind:show={showWhatsNew}
	version={whatsNewVersion}
	releaseUrl={whatsNewReleaseUrl}
/>

<Toast
	bind:show={showUpToDateToast}
	message="You're running the latest version of Nebo."
	type="success"
	duration={3000}
/>
