<script lang="ts">
	import {
		updateInfo,
		updateDismissed,
		downloadProgress,
		updateReady,
		updateError,
		applyUpdate
	} from '$lib/stores/update';
	import { ArrowUpCircle, X, RefreshCw, Download } from 'lucide-svelte';

	let show = $derived(
		($updateInfo?.available || $downloadProgress || $updateReady || $updateError) && !$updateDismissed
	);

	let isApplying = $state(false);

	function dismiss() {
		updateDismissed.set(true);
	}

	async function handleApply() {
		isApplying = true;
		await applyUpdate();
	}
</script>

{#if show}
	<div class="alert alert-info shadow-lg mx-4 mt-2 mb-0 flex items-center gap-3 py-2 px-4 text-sm">
		{#if $updateReady}
			<!-- Update downloaded and verified, ready to install -->
			<RefreshCw class="w-5 h-5 shrink-0" />
			<div class="flex-1 min-w-0">
				<span class="font-semibold">Nebo {$updateReady}</span> is ready to install
			</div>
			<button
				class="btn btn-sm btn-primary"
				onclick={handleApply}
				disabled={isApplying}
			>
				{#if isApplying}
					<span class="loading loading-spinner loading-xs"></span>
					Restarting...
				{:else}
					Restart to Update
				{/if}
			</button>
		{:else if $downloadProgress}
			<!-- Download in progress -->
			<Download class="w-5 h-5 shrink-0 animate-pulse" />
			<div class="flex-1 min-w-0 flex items-center gap-3">
				<span>Downloading update...</span>
				<progress class="progress progress-info w-32" value={$downloadProgress.percent} max="100"></progress>
				<span class="text-info-content/60 tabular-nums">{$downloadProgress.percent}%</span>
			</div>
		{:else if $updateError}
			<!-- Download or verification failed -->
			<ArrowUpCircle class="w-5 h-5 shrink-0" />
			<div class="flex-1 min-w-0">
				<span class="font-semibold">Update failed:</span>
				<span class="text-info-content/60">{$updateError}</span>
			</div>
		{:else if $updateInfo}
			<!-- Update available (notification only for package manager installs) -->
			<ArrowUpCircle class="w-5 h-5 shrink-0" />
			<div class="flex-1 min-w-0">
				<span class="font-semibold">Nebo {$updateInfo.latest_version}</span> is available
				<span class="text-info-content/60">(you're on {$updateInfo.current_version})</span>
				{#if $updateInfo.install_method === 'homebrew'}
					<span class="text-info-content/60 ml-1">— run <code>brew upgrade nebo</code></span>
				{:else if $updateInfo.install_method === 'package_manager'}
					<span class="text-info-content/60 ml-1">— run <code>sudo apt upgrade nebo</code></span>
				{/if}
			</div>
			{#if $updateInfo.release_url}
				<a
					href={$updateInfo.release_url}
					target="_blank"
					rel="noopener noreferrer"
					class="btn btn-sm btn-ghost"
				>
					Release notes
				</a>
			{/if}
		{/if}
		<button class="btn btn-sm btn-ghost btn-square" onclick={dismiss} aria-label="Dismiss">
			<X class="w-4 h-4" />
		</button>
	</div>
{/if}
