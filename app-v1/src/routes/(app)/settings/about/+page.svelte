<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import {
		updateInfo,
		checkForUpdate,
		resetUpdateState,
		downloadProgress,
		updateReady,
		updateError,
		autoUpdateEnabled,
		applyUpdate
	} from '$lib/stores/update';
	import { getWebSocketClient } from '$lib/websocket/client';
	import Spinner from '$lib/components/ui/Spinner.svelte';
	import { ExternalLink, CheckCircle, ArrowUpCircle, AlertCircle } from 'lucide-svelte';
	import { t } from 'svelte-i18n';

	let isChecking = $state(false);
	let isApplying = $state(false);
	let checkResult = $state<'none' | 'up-to-date' | 'available'>('none');
	let unsubs: Array<() => void> = [];

	// Ensure version/install info is loaded when visiting this page
	onMount(() => {
		if (!$updateInfo) {
			checkForUpdate();
		}

		// Listen for update errors from the backend
		const ws = getWebSocketClient();
		unsubs.push(
			ws.on('update_error', (data: { error: string }) => {
				updateError.set(data.error || $t('settingsAbout.updateFailed'));
				isApplying = false;
			}),
		);
	});

	onDestroy(() => unsubs.forEach(fn => fn()));

	// Auto-apply when update is downloaded and auto-update is on
	$effect(() => {
		if ($updateReady && $autoUpdateEnabled) {
			handleApply();
		}
	});

	$effect(() => {
		if ($updateError) isApplying = false;
	});

	async function handleCheck() {
		isChecking = true;
		checkResult = 'none';
		resetUpdateState();
		await checkForUpdate();
		isChecking = false;
		checkResult = $updateInfo?.available ? 'available' : 'up-to-date';
	}

	async function handleApply() {
		isApplying = true;
		await applyUpdate();
	}

	function handleRetry() {
		updateError.set(null);
		checkForUpdate();
	}

	const resourceLinks = [
		{ label: 'settingsAbout.learn', href: 'https://getnebo.com/learn' },
		{ label: 'nav.marketplace', href: 'https://neboloop.com' },
		{ label: 'settingsAbout.sendFeedback', href: 'https://getnebo.com/support/feedback' },
		{ label: 'settingsAbout.developers', href: 'https://getnebo.com/developers' },
		{ label: 'settingsAbout.github', href: 'https://github.com/NeboLoop/nebo' }
	];

	function getPlatform(): string {
		const ua = navigator.userAgent.toLowerCase();
		if (ua.includes('mac')) return 'macOS';
		if (ua.includes('win')) return 'Windows';
		if (ua.includes('linux')) return 'Linux';
		return navigator.platform;
	}
</script>

<div class="mb-6">
	<h2 class="font-display text-xl font-bold text-base-content mb-1">{$t('settingsAbout.title')}</h2>
	<p class="text-base text-base-content/80">{$t('settingsAbout.description')}</p>
</div>

<div class="space-y-6">
	<!-- App Identity -->
	<section>
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			<div class="flex items-center gap-4">
				<div class="w-14 h-14 rounded-2xl bg-primary/10 flex items-center justify-center shrink-0">
					<img
						src="/nebo-icon.svg"
						alt="Nebo"
						class="w-9 h-9"
						onerror={(e: Event) => {
							(e.currentTarget as HTMLElement).style.display = 'none';
						}}
					/>
				</div>
				<div>
					<h3 class="font-display text-lg font-bold text-base-content">Nebo</h3>
					<p class="text-sm text-base-content/60">{$t('settingsAbout.personalDesktopAI')}</p>
				</div>
			</div>
			<div class="mt-4 space-y-2">
				{#each [[`${$t('settingsAbout.version')}`, $updateInfo?.currentVersion ?? '—'], [$t('settingsAbout.platform'), getPlatform()], [$t('settingsAbout.installMethod'), ($updateInfo?.installMethod ?? '—').replace('_', ' ')], [$t('settingsAbout.license'), $t('settingsAbout.apache')]] as [label, value]}
					<div class="flex items-center justify-between py-1">
						<span class="text-sm text-base-content/60">{label}</span>
						<span class="text-sm text-base-content font-medium">{value}</span>
					</div>
				{/each}
			</div>
		</div>
	</section>

	<!-- Software Update -->
	<section>
		<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">
			{$t('settingsAbout.softwareUpdate')}
		</h3>
		<div class="rounded-2xl bg-base-200/50 border border-base-content/10 p-5">
			{#if $downloadProgress}
				<!-- Downloading -->
				<div class="flex items-center gap-3">
					<Spinner size={20} />
					<div class="flex-1 min-w-0">
						<p class="text-base font-medium text-base-content">{$t('settingsAbout.downloadingUpdate')}</p>
						<div class="mt-2 h-1.5 rounded-full bg-base-content/10 overflow-hidden">
							<div
								class="h-full rounded-full bg-primary transition-all"
								style="width: {$downloadProgress.percent}%"
							></div>
						</div>
						<p class="text-sm text-base-content/60 mt-1 tabular-nums">
							{$t('settingsAbout.percent', { values: { percent: $downloadProgress.percent } })}
						</p>
					</div>
				</div>
			{:else if isApplying}
				<!-- Applying -->
				<div class="flex items-center gap-3">
					<Spinner size={20} />
					<div>
						<p class="text-base font-medium text-base-content">{$t('settingsAbout.installingUpdate')}</p>
						<p class="text-sm text-base-content/60">{$t('settingsAbout.restartMomentarily')}</p>
					</div>
				</div>
			{:else if $updateError}
				<!-- Error -->
				<div class="flex items-center gap-3">
					<AlertCircle class="w-5 h-5 text-error shrink-0" />
					<div class="flex-1 min-w-0">
						<p class="text-base font-medium text-base-content">{$t('settingsAbout.updateFailed')}</p>
						<p class="text-sm text-error/80 mt-0.5">{$updateError}</p>
					</div>
					<button
						type="button"
						class="h-9 px-5 rounded-full border border-base-content/10 text-sm font-medium hover:bg-base-content/5 transition-colors"
						onclick={handleRetry}
					>
						{$t('common.retry')}
					</button>
				</div>
			{:else if $updateReady}
				<!-- Ready to install -->
				<div class="flex items-center gap-3">
					<ArrowUpCircle class="w-5 h-5 text-info shrink-0" />
					<div class="flex-1 min-w-0">
						<p class="text-base font-medium text-base-content">{$t('settingsAbout.updateReady', { values: { version: $updateReady } })}</p>
						<p class="text-sm text-base-content/60">{$t('settingsAbout.downloadedVerified')}</p>
					</div>
					<button
						type="button"
						class="h-9 px-5 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all"
						onclick={handleApply}
					>
						{$t('settingsAbout.restartUpdate')}
					</button>
				</div>
			{:else if $updateInfo?.available}
				<!-- Update available -->
				<div class="flex items-center gap-3">
					<ArrowUpCircle class="w-5 h-5 text-info shrink-0" />
					<div class="flex-1 min-w-0">
						<p class="text-base font-medium text-base-content">
							{$t('settingsAbout.updateAvailable', { values: { version: $updateInfo.latestVersion } })}
						</p>
						{#if $updateInfo.installMethod === 'homebrew'}
							<p class="text-sm text-base-content/60">
								{$t('settingsAbout.brewUpgrade')}
							</p>
						{:else if $updateInfo.installMethod === 'package_manager'}
							<p class="text-sm text-base-content/60">
								{$t('settingsAbout.aptUpgrade')}
							</p>
						{:else}
							<p class="text-sm text-base-content/60">{$t('settingsAbout.newerAvailable')}</p>
						{/if}
					</div>
					{#if $updateInfo.canAutoUpdate}
						<button
							type="button"
							class="h-9 px-5 rounded-full bg-primary text-primary-content text-sm font-bold hover:brightness-110 transition-all"
							onclick={handleApply}
						>
							{$t('settingsAbout.updateNow')}
						</button>
					{/if}
				</div>
			{:else if checkResult === 'up-to-date'}
				<!-- Up to date -->
				<div class="flex items-center gap-3">
					<CheckCircle class="w-5 h-5 text-success shrink-0" />
					<div class="flex-1 min-w-0">
						<p class="text-base font-medium text-base-content">{$t('settingsAbout.upToDate')}</p>
						<p class="text-sm text-base-content/60">{$t('settingsAbout.runningLatest')}</p>
					</div>
					<button
						type="button"
						class="text-sm text-base-content/60 hover:text-primary transition-colors"
						onclick={handleCheck}
						disabled={isChecking}
					>
						{$t('settingsAbout.checkAgain')}
					</button>
				</div>
			{:else}
				<!-- Default: check for updates -->
				<div class="flex items-center gap-3">
					<div class="flex-1 min-w-0">
						<p class="text-base text-base-content/80">{$t('settingsAbout.checkDescription')}</p>
					</div>
					<button
						type="button"
						class="h-9 px-5 rounded-full border border-base-content/10 text-sm font-medium hover:bg-base-content/5 transition-colors disabled:opacity-50"
						onclick={handleCheck}
						disabled={isChecking}
					>
						{#if isChecking}<Spinner size={14} />{:else}{$t('settingsAbout.checkForUpdates')}{/if}
					</button>
				</div>
			{/if}
		</div>
	</section>

	<!-- Links -->
	<section>
		<h3 class="text-base font-semibold text-base-content/60 uppercase tracking-wider mb-3">
			{$t('settingsAbout.resources')}
		</h3>
		<div
			class="rounded-2xl bg-base-200/50 border border-base-content/10 divide-y divide-base-content/10"
		>
			{#each resourceLinks as link}
				<a
					href={link.href}
					target="_blank"
					rel="noopener noreferrer"
					class="flex items-center justify-between px-5 py-3 text-base text-base-content hover:bg-base-content/5 transition-colors"
				>
					<span>{$t(link.label)}</span>
					<ExternalLink class="w-4 h-4 text-base-content/40" />
				</a>
			{/each}
		</div>
	</section>
</div>
