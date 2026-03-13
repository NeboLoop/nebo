<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { AppNav } from '$lib/components/navigation';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { auth } from '$lib/stores/auth';
	import { get } from 'svelte/store';
	import OnboardingFlow from '$lib/components/onboarding/OnboardingFlow.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import UpdateBanner from '$lib/components/UpdateBanner.svelte';
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

	let { children }: { children: Snippet } = $props();

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
		$page.url.pathname.startsWith('/store')
	);

	// Settings renders as a centered modal overlay
	const isSettingsRoute = $derived(
		$page.url.pathname.startsWith('/settings')
	);

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
		};
	});

	function dismissQuarantine(appId: string) {
		quarantineNotices = quarantineNotices.filter(n => n.app_id !== appId);
	}
</script>

<!-- Show onboarding if needed -->
{#if showOnboarding && !isCheckingOnboarding}
	<OnboardingFlow />
{:else if isCheckingOnboarding}
	<!-- Loading state while checking onboarding -->
	<div class="h-dvh flex items-center justify-center bg-base-100">
		<div class="loading loading-spinner loading-lg"></div>
	</div>
{:else if isFullHeightRoute}
	<div class="h-dvh flex flex-col overflow-hidden bg-base-100">
		<AppNav />
		<UpdateBanner />
		{#each quarantineNotices as notice (notice.app_id)}
			<div class="px-4 pt-2">
				<Alert type="warning" title="{notice.app_name} was removed due to a security concern." dismissible onclose={() => dismissQuarantine(notice.app_id)}>
					Your data is safe. This app was automatically stopped and quarantined by NeboLoop.
				</Alert>
			</div>
		{/each}
		<main id="main-content" class="flex-1 flex flex-col min-h-0 overflow-hidden">
			{@render children()}
		</main>
	</div>
{:else if isSettingsRoute}
	<div class="layout-app h-full">
		<AppNav />
		<UpdateBanner />
		{#each quarantineNotices as notice (notice.app_id)}
			<div class="px-6 pt-2">
				<Alert type="warning" title="{notice.app_name} was removed due to a security concern." dismissible onclose={() => dismissQuarantine(notice.app_id)}>
					Your data is safe. This app was automatically stopped and quarantined by NeboLoop.
				</Alert>
			</div>
		{/each}
		{@render children()}
	</div>
{:else if isEdgeToEdgeRoute}
	<div class="layout-app h-full">
		<AppNav />
		<UpdateBanner />
		{#each quarantineNotices as notice (notice.app_id)}
			<div class="px-6 pt-2">
				<Alert type="warning" title="{notice.app_name} was removed due to a security concern." dismissible onclose={() => dismissQuarantine(notice.app_id)}>
					Your data is safe. This app was automatically stopped and quarantined by NeboLoop.
				</Alert>
			</div>
		{/each}
		<main id="main-content" class="flex-1 px-6 pt-6">
			{@render children()}
		</main>
	</div>
{:else}
	<div class="layout-app h-full">
		<AppNav />
		<UpdateBanner />
		{#each quarantineNotices as notice (notice.app_id)}
			<div class="px-6 pt-2">
				<Alert type="warning" title="{notice.app_name} was removed due to a security concern." dismissible onclose={() => dismissQuarantine(notice.app_id)}>
					Your data is safe. This app was automatically stopped and quarantined by NeboLoop.
				</Alert>
			</div>
		{/each}
		<main id="main-content" class="flex-1 p-6">
			<div class="max-w-[1400px] mx-auto">
				{@render children()}
			</div>
		</main>
	</div>
{/if}
