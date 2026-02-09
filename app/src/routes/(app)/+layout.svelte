<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { AppNav } from '$lib/components/navigation';
	import { getWebSocketClient } from '$lib/websocket/client';
	import OnboardingFlow from '$lib/components/onboarding/OnboardingFlow.svelte';
	import Alert from '$lib/components/ui/Alert.svelte';
	import { auth } from '$lib/stores/auth';
	import * as api from '$lib/api/nebo';

	let { children }: { children: Snippet } = $props();

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

	onMount(async () => {
		// Connect WebSocket with user ID for proper session scoping
		const authState = get(auth);
		const userId = authState.user?.id || 'anonymous';
		const wsClient = getWebSocketClient();
		wsClient.connect(userId);

		// Listen for quarantine events from the agent
		const unsubQuarantine = wsClient.on<QuarantineNotice>('app_quarantined', (data) => {
			if (data) {
				quarantineNotices = [...quarantineNotices, data];
			}
		});

		// Check if user needs onboarding
		try {
			const response = await api.getUserProfile();
			showOnboarding = !response.profile?.onboardingCompleted;
		} catch (err) {
			// If we can't get profile, show onboarding
			showOnboarding = true;
		} finally {
			isCheckingOnboarding = false;
		}

		return () => {
			unsubQuarantine();
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
{:else}
	<div class="layout-app h-full">
		<AppNav />
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
