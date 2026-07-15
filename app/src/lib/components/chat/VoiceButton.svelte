<script lang="ts">
	import { dictationStore, type DictationRoute } from '$lib/stores/dictation';
	import { voiceStore } from '$lib/stores/voice';
	import { deviceManager, micDevices, selectedMicId } from '$lib/stores/devices';
	import Mic from 'lucide-svelte/icons/mic';
	import MicOff from 'lucide-svelte/icons/mic-off';
	import Volume2 from 'lucide-svelte/icons/volume-2';
	import ChevronDown from 'lucide-svelte/icons/chevron-down';
	import Check from 'lucide-svelte/icons/check';

	let { ownerId, route, onrecordstart, onstop }: {
		ownerId: string;
		route?: DictationRoute;
		onrecordstart?: () => void;
		onstop?: () => void;
	} = $props();

	let status = $derived($dictationStore.status);
	let error = $derived($dictationStore.error);
	let isPushToTalk = $derived($dictationStore.isPushToTalk);
	let holdToRecordEnabled = $derived($dictationStore.holdToRecordEnabled);
	let isPlaying = $derived($voiceStore.isPlaying);

	let isActive = $derived(status === 'recording' || status === 'connecting');
	let isRecording = $derived(status === 'recording');
	let isConnecting = $derived(status === 'connecting');

	// Dropdown state
	let dropdownOpen = $state(false);
	let dropdownRef: HTMLDivElement | null = $state(null);

	// Push-to-talk / hold-to-record state
	let holdTimer: ReturnType<typeof setTimeout> | null = $state(null);
	let holdActivated = $state(false);

	// Refresh mic list when dropdown opens
	$effect(() => {
		if (dropdownOpen) {
			deviceManager.refresh();
		}
	});

	// Click-outside to close dropdown
	$effect(() => {
		if (!dropdownOpen) return;

		function handleClickOutside(e: PointerEvent) {
			if (dropdownRef && !dropdownRef.contains(e.target as Node)) {
				dropdownOpen = false;
			}
		}

		document.addEventListener('pointerdown', handleClickOutside, true);
		return () => {
			document.removeEventListener('pointerdown', handleClickOutside, true);
		};
	});

	function handleStopPlayback() {
		voiceStore.stopPlayback();
	}

	async function startRecording() {
		await dictationStore.start(ownerId, route ?? { type: 'editor' });
		onrecordstart?.();
	}

	function stopRecording() {
		dictationStore.stop();
		onstop?.();
	}

	function handlePointerDown() {
		if (isActive) {
			// Already active: stop on click
			stopRecording();
			return;
		}

		if (!holdToRecordEnabled) {
			// Toggle mode: just start, pointerup won't stop
			return;
		}

		// Hold-to-record enabled: start recording and begin hold timer
		holdActivated = false;
		startRecording();

		holdTimer = setTimeout(() => {
			holdActivated = true;
			dictationStore.setPushToTalk(true);
		}, 500);
	}

	function handlePointerUp() {
		// Clear the hold timer if still pending
		if (holdTimer) {
			clearTimeout(holdTimer);
			holdTimer = null;
		}

		if (!holdToRecordEnabled) {
			// Toggle mode: pointerup does nothing
			return;
		}

		if (holdActivated) {
			// Held long enough: stop recording on release
			holdActivated = false;
			stopRecording();
		}
		// Short click (< 500ms) with hold-to-record: recording already started, let it run as toggle
	}

	async function handleClick() {
		if (holdToRecordEnabled) {
			// Handled by pointerdown/pointerup
			return;
		}

		// Toggle mode
		if (isActive) {
			stopRecording();
		} else {
			await startRecording();
		}
	}

	function handleSelectMic(deviceId: string) {
		deviceManager.selectMic(deviceId);
	}

	function handleToggleHoldToRecord() {
		dictationStore.setHoldToRecordEnabled(!holdToRecordEnabled);
	}

	function toggleDropdown(e: MouseEvent) {
		e.stopPropagation();
		dropdownOpen = !dropdownOpen;
	}

	function micLabel(device: MediaDeviceInfo): string {
		return device.label || `Microphone ${device.deviceId.slice(0, 8)}`;
	}
</script>

{#if isPlaying}
	<button
		class="w-8 h-8 rounded-lg grid place-items-center text-base-content/60 hover:text-base-content hover:bg-base-200 cursor-pointer transition-colors border-none bg-transparent"
		title="Stop playback"
		onclick={handleStopPlayback}
	>
		<Volume2 class="w-[1.125rem] h-[1.125rem] text-primary voice-pulse" />
	</button>
{:else}
	<div class="relative flex items-center" bind:this={dropdownRef}>
		<!-- Main mic button -->
		<button
			class="w-8 h-8 rounded-lg grid place-items-center cursor-pointer transition-colors border-none bg-transparent {isRecording ? 'text-error' : isConnecting ? 'text-warning' : 'text-base-content/60 hover:text-base-content hover:bg-base-200'}"
			title={isRecording
				? isPushToTalk ? 'Release to stop' : 'Stop recording'
				: isConnecting ? 'Connecting...'
				: holdToRecordEnabled ? 'Hold to record' : 'Start voice input'}
			onclick={handleClick}
			onpointerdown={handlePointerDown}
			onpointerup={handlePointerUp}
			onpointerleave={handlePointerUp}
		>
			{#if isRecording}
				<MicOff class="w-[1.125rem] h-[1.125rem] animate-pulse" />
			{:else if isConnecting}
				<Mic class="w-[1.125rem] h-[1.125rem] animate-pulse" />
			{:else}
				<Mic class="w-[1.125rem] h-[1.125rem]" />
			{/if}
		</button>

		<!-- PTT indicator -->
		{#if isPushToTalk && isRecording}
			<span class="text-xs text-error/80 font-medium ml-0.5">PTT</span>
		{/if}

		<!-- Chevron dropdown trigger -->
		<button
			class="w-4 h-6 grid place-items-center text-base-content/40 hover:text-base-content/70 cursor-pointer transition-colors border-none bg-transparent -ml-1"
			title="Voice settings"
			onclick={toggleDropdown}
		>
			<ChevronDown class="w-3 h-3" />
		</button>

		<!-- Dropdown popover -->
		{#if dropdownOpen}
			<div class="absolute bottom-full left-0 mb-2 w-64 rounded-lg bg-base-100 border border-base-300 shadow-lg z-50">
				<!-- Microphone selection -->
				<div class="p-2">
					<div class="text-xs font-semibold uppercase tracking-wider text-base-content/50 px-2 pb-1">Microphone</div>
					{#each $micDevices as device (device.deviceId)}
						<button
							class="w-full flex items-center gap-2 px-2 py-1.5 rounded-md text-sm text-base-content hover:bg-base-200/50 cursor-pointer border-none bg-transparent text-left"
							onclick={() => handleSelectMic(device.deviceId)}
						>
							<span class="w-4 h-4 grid place-items-center shrink-0">
								{#if $selectedMicId === device.deviceId}
									<Check class="w-3.5 h-3.5 text-primary" />
								{/if}
							</span>
							<span class="truncate">{micLabel(device)}</span>
						</button>
					{:else}
						<div class="px-2 py-1.5 text-xs text-base-content/50">No microphones found</div>
					{/each}
				</div>

				<!-- Divider -->
				<div class="border-t border-base-content/10"></div>

				<!-- Hold to record toggle -->
				<div class="p-2">
					<button
						class="w-full flex items-center justify-between px-2 py-1.5 rounded-md text-sm text-base-content hover:bg-base-200/50 cursor-pointer border-none bg-transparent"
						onclick={handleToggleHoldToRecord}
					>
						<span>Hold to record</span>
						<input
							type="checkbox"
							class="toggle toggle-sm toggle-primary"
							checked={holdToRecordEnabled}
							tabindex={-1}
							onclick={(e) => e.stopPropagation()}
							onchange={handleToggleHoldToRecord}
						/>
					</button>
				</div>
			</div>
		{/if}
	</div>
{/if}

{#if error && isActive}
	<span class="text-xs text-error ml-1">{error}</span>
{/if}
