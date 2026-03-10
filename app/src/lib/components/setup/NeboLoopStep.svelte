<!--
  NeboLoop Onboarding Step
  OAuth popup login via NeboLoop.com — supports Google, Apple, email/password
  Uses DaisyUI components
-->

<script lang="ts">
	import { StepCard, StepNavigation } from '$lib/components/setup';
	import { neboLoopOAuthStart, neboLoopOAuthStatus, neboLoopAccountStatus } from '$lib/api';
	import { CircleCheck, ExternalLink, LoaderCircle, Store } from 'lucide-svelte';

	let {
		onback,
		onnext,
		onskip
	}: {
		onback?: () => void;
		onnext?: () => void;
		onskip?: () => void;
	} = $props();

	let loading = $state(false);
	let error = $state('');
	let connected = $state(false);
	let connectedEmail = $state('');
	let pendingState = $state('');
	let pollTimer = $state<ReturnType<typeof setInterval> | null>(null);

	// Check if already connected on mount
	$effect(() => {
		checkStatus();
	});

	// Clean up on unmount
	$effect(() => {
		return () => {
			cleanup();
		};
	});

	async function checkStatus() {
		try {
			const status = await neboLoopAccountStatus();
			if (status.connected) {
				connected = true;
				connectedEmail = status.email ?? '';
			}
		} catch {
			// Not connected, that's fine
		}
	}

	async function startOAuth() {
		error = '';
		loading = true;
		try {
			const { state } = await neboLoopOAuthStart();
			pendingState = state;

			// Auto-timeout after 3 minutes
			const timeout = setTimeout(() => {
				if (loading) {
					cleanup();
					error = 'Sign-in timed out. Please try again.';
					loading = false;
				}
			}, 3 * 60 * 1000);

			// Poll status until the OAuth flow completes in the browser
			pollTimer = setInterval(async () => {
				try {
					const result = await neboLoopOAuthStatus({ state: pendingState });
					if (result.status === 'complete') {
						clearTimeout(timeout);
						cleanup();
						connected = true;
						connectedEmail = result.email ?? '';
						loading = false;
					} else if (result.status === 'error') {
						clearTimeout(timeout);
						cleanup();
						error = result.error ?? 'Sign-in failed';
						loading = false;
					} else if (result.status === 'expired') {
						clearTimeout(timeout);
						cleanup();
						error = 'Sign-in expired. Please try again.';
						loading = false;
					}
				} catch {
					// polling error, keep trying
				}
			}, 2000);
		} catch (e: any) {
			error = e?.message || 'Failed to start sign-in';
			loading = false;
		}
	}

	function cleanup() {
		if (pollTimer) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
		pendingState = '';
	}
</script>

<StepCard
	title="NeboLoop"
	description="Connect to the NeboLoop marketplace to install apps, skills, and AI providers for your agent."
>
	{#if connected}
		<div class="flex flex-col items-center gap-4 py-6">
			<CircleCheck class="h-12 w-12 text-success" />
			<p class="text-lg font-medium">Connected to NeboLoop</p>
			<p class="text-base-content/70">{connectedEmail}</p>
		</div>
		<StepNavigation showBack onnext={onnext} onback={onback} nextLabel="Continue" />
	{:else}
		{#if error}
			<div class="alert alert-error mb-4">
				<span>{error}</span>
			</div>
		{/if}

		<div class="flex flex-col items-center gap-6 py-6">
			<Store class="h-16 w-16 text-primary opacity-80" />
			<p class="text-base-content/70 text-center text-sm max-w-sm">
				Sign in or create a new account on NeboLoop.
				You can use Google, Apple, or email.
			</p>
			<button
				type="button"
				class="btn btn-primary btn-lg gap-2"
				onclick={startOAuth}
				disabled={loading}
			>
				{#if loading}
					<LoaderCircle class="h-5 w-5 animate-spin" />
					Waiting for sign-in...
				{:else}
					<ExternalLink class="h-5 w-5" />
					Continue with NeboLoop
				{/if}
			</button>
			{#if loading}
				<p class="text-sm text-base-content/50">Complete sign-in in your browser</p>
				<button
					type="button"
					class="text-sm text-base-content/50 hover:text-base-content underline"
					onclick={() => { cleanup(); loading = false; }}
				>
					Cancel
				</button>
			{/if}
		</div>

		<StepNavigation
			showBack
			showSkip
			onback={onback}
			onskip={onskip}
		/>
	{/if}
</StepCard>
