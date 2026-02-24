<script lang="ts">
	import {
		updateInfo,
		updateDismissed,
		downloadProgress,
		updateReady,
		updateError,
		applyUpdate,
		checkForUpdate
	} from '$lib/stores/update';
	import { ArrowUpCircle, X, Download } from 'lucide-svelte';

	let show = $derived(
		($updateInfo?.available || $downloadProgress || $updateReady || $updateError) && !$updateDismissed
	);

	let isApplying = $state(false);

	// Auto-apply as soon as the update is ready (downloaded + verified).
	// No isApplying guard — if the user already clicked "Update Now" before
	// the download finished, we need to re-trigger applyUpdate once the
	// binary is actually staged.
	$effect(() => {
		if ($updateReady) {
			handleApply();
		}
	});

	// Reset applying state when an error arrives so the error UI is visible
	$effect(() => {
		if ($updateError) {
			isApplying = false;
		}
	});

	function dismiss() {
		updateDismissed.set(true);
	}

	async function handleApply() {
		isApplying = true;
		await applyUpdate();
	}

	function handleRetry() {
		updateError.set(null);
		checkForUpdate();
	}
</script>

{#if show}
	<div class="alert alert-info shadow-lg mx-4 mt-2 mb-0 flex items-center gap-3 py-2 px-4 text-sm">
		{#if $downloadProgress}
			<!-- Download in progress — always visible even if user clicked Update Now -->
			<Download class="w-5 h-5 shrink-0 animate-pulse" />
			<div class="flex-1 min-w-0 flex items-center gap-3">
				<span>Downloading update...</span>
				<progress class="progress progress-info w-32" value={$downloadProgress.percent} max="100"></progress>
				<span class="text-info-content/60 tabular-nums">{$downloadProgress.percent}%</span>
			</div>
		{:else if $updateError}
			<!-- Download or verification failed — always visible -->
			<ArrowUpCircle class="w-5 h-5 shrink-0" />
			<div class="flex-1 min-w-0">
				<span class="font-semibold">Update failed</span>
				<span class="text-info-content/60">— please try again</span>
			</div>
			<button class="btn btn-sm btn-primary" onclick={handleRetry}>
				Retry
			</button>
		{:else if isApplying}
			<!-- Binary staged, applying + restarting -->
			<span class="loading loading-spinner loading-sm shrink-0"></span>
			<div class="flex-1 min-w-0">
				Updating Nebo — this will only take a moment...
			</div>
		{:else if $updateInfo?.available}
			<!-- Update available -->
			<ArrowUpCircle class="w-5 h-5 shrink-0" />
			<div class="flex-1 min-w-0">
				<span class="font-semibold">Nebo {$updateInfo.latest_version}</span> is available
				{#if $updateInfo.install_method === 'homebrew'}
					<span class="text-info-content/60 ml-1">— run <code>brew upgrade nebo</code></span>
				{:else if $updateInfo.install_method === 'package_manager'}
					<span class="text-info-content/60 ml-1">— run <code>sudo apt upgrade nebo</code></span>
				{/if}
			</div>
			{#if $updateInfo.can_auto_update}
				<button class="btn btn-sm btn-primary" onclick={handleApply}>
					Update Now
				</button>
			{/if}
		{/if}
		<button class="btn btn-sm btn-ghost btn-square" onclick={dismiss} aria-label="Dismiss">
			<X class="w-4 h-4" />
		</button>
	</div>
{/if}
