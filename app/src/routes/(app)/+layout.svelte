<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { get } from 'svelte/store';
	import { AppNav } from '$lib/components/navigation';
	import { getWebSocketClient } from '$lib/websocket/client';
	import OnboardingFlow from '$lib/components/onboarding/OnboardingFlow.svelte';
	import { auth } from '$lib/stores/auth';
	import * as api from '$lib/api/nebo';

	let { children }: { children: Snippet } = $props();

	// Onboarding state
	let isCheckingOnboarding = $state(true);
	let showOnboarding = $state(false);

	// Full-height pages that need flex layout
	const isFullHeightRoute = $derived(
		$page.url.pathname.startsWith('/agent') ||
		$page.url.pathname.startsWith('/settings/heartbeat')
	);

	onMount(async () => {
		// Connect WebSocket with user ID for proper session scoping
		const authState = get(auth);
		const userId = authState.user?.id || 'anonymous';
		getWebSocketClient().connect(userId);

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
	});
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
		<main id="main-content" class="flex-1 flex flex-col min-h-0 overflow-hidden p-6">
			<div class="max-w-[1400px] mx-auto w-full flex-1 flex flex-col min-h-0">
				{@render children()}
			</div>
		</main>
	</div>
{:else}
	<div class="layout-app h-full">
		<AppNav />
		<main id="main-content" class="flex-1 p-6">
			<div class="max-w-[1400px] mx-auto">
				{@render children()}
			</div>
		</main>
	</div>
{/if}
