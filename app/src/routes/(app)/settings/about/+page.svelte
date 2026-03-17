<script lang="ts">
	import { updateInfo, checkForUpdate, resetUpdateState } from '$lib/stores/update';
	import { ExternalLink, RefreshCw } from 'lucide-svelte';

	let isChecking = $state(false);
	let checkResult = $state<'none' | 'up-to-date' | 'available'>('none');

	async function handleCheckForUpdate() {
		isChecking = true;
		checkResult = 'none';
		resetUpdateState();
		await checkForUpdate();
		isChecking = false;
		// After check, see if an update is available
		const info = $updateInfo;
		checkResult = info?.available ? 'available' : 'up-to-date';
	}

	function getPlatform(): string {
		const ua = navigator.userAgent.toLowerCase();
		if (ua.includes('mac')) return 'macOS';
		if (ua.includes('win')) return 'Windows';
		if (ua.includes('linux')) return 'Linux';
		return navigator.platform;
	}
</script>

<div class="space-y-8">
	<!-- Header -->
	<div class="flex items-center gap-4">
		<div class="w-16 h-16 rounded-2xl bg-primary/10 flex items-center justify-center">
			<img src="/nebo-icon.svg" alt="Nebo" class="w-10 h-10" onerror={(e: Event) => { (e.currentTarget as HTMLElement).style.display = 'none'; }} />
		</div>
		<div>
			<h2 class="text-2xl font-bold text-base-content">Nebo</h2>
			<p class="text-base-content/60">Personal Desktop AI Companion</p>
		</div>
	</div>

	<!-- Info table -->
	<div class="overflow-hidden rounded-lg border border-base-content/10">
		<table class="table table-sm w-full">
			<tbody>
				<tr>
					<td class="font-medium text-base-content/70 w-40">Version</td>
					<td class="text-base-content">{$updateInfo?.currentVersion ?? '—'}</td>
				</tr>
				<tr>
					<td class="font-medium text-base-content/70">License</td>
					<td class="text-base-content">Apache 2.0</td>
				</tr>
				<tr>
					<td class="font-medium text-base-content/70">Copyright</td>
					<td class="text-base-content">2026 Nebo LLC</td>
				</tr>
				<tr>
					<td class="font-medium text-base-content/70">Install Method</td>
					<td class="text-base-content capitalize">{$updateInfo?.installMethod ?? '—'}</td>
				</tr>
				<tr>
					<td class="font-medium text-base-content/70">Platform</td>
					<td class="text-base-content">{getPlatform()}</td>
				</tr>
			</tbody>
		</table>
	</div>

	<!-- Check for Updates -->
	<div class="space-y-2">
		<button
			class="btn btn-outline btn-sm gap-2"
			onclick={handleCheckForUpdate}
			disabled={isChecking}
		>
			<RefreshCw class="w-4 h-4 {isChecking ? 'animate-spin' : ''}" />
			{isChecking ? 'Checking...' : 'Check for Updates'}
		</button>
		{#if checkResult === 'up-to-date'}
			<p class="text-sm text-success">You're running the latest version.</p>
		{:else if checkResult === 'available'}
			<p class="text-sm text-info">A new version is available — see the banner above.</p>
		{/if}
	</div>

	<!-- Links -->
	<div class="flex flex-wrap gap-4">
		{#each [
			{ label: 'Website', href: 'https://neboloop.com' },
			{ label: 'GitHub', href: 'https://github.com/NeboLoop/nebo' },
			{ label: 'Documentation', href: 'https://neboloop.com/docs' },
			{ label: 'Send Feedback', href: 'https://github.com/NeboLoop/nebo/issues' }
		] as link}
			<a
				href={link.href}
				target="_blank"
				rel="noopener noreferrer"
				class="inline-flex items-center gap-1 text-sm text-primary hover:text-primary/80 transition-colors"
			>
				{link.label}
				<ExternalLink class="w-3 h-3" />
			</a>
		{/each}
	</div>
</div>
