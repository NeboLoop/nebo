/**
 * Voice device manager — enumerates microphones, persists selection,
 * handles hot-plug and OverconstrainedError fallback.
 * One store owns the device list and the chosen microphone.
 */

import { writable, derived } from 'svelte/store';
import { storage } from '$lib/storage';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'VoiceDeviceManager' });
const STORAGE_KEY = 'voice:selected-mic-device-id';

/** What the microphone is for. `voice` is a live conversation: 16 kHz with
 *  echo cancellation, since the employee's voice plays back while the owner
 *  speaks. `clip` is a recording the owner keeps (a voiceover, a memo): the
 *  device's own rate and no echo cancellation, which would thin the voice. */
export type MicPurpose = 'voice' | 'clip';

export interface DeviceManagerState {
	inputs: MediaDeviceInfo[];
	outputs: MediaDeviceInfo[];
	selectedMicId: string | null;
}

const initialState: DeviceManagerState = {
	inputs: [],
	outputs: [],
	selectedMicId: storage.get(STORAGE_KEY)
};

function createDeviceManager() {
	const { subscribe, update } = writable<DeviceManagerState>(initialState);

	async function refresh() {
		try {
			const devices = await navigator.mediaDevices.enumerateDevices();
			update((s) => ({
				...s,
				inputs: devices.filter((d) => d.kind === 'audioinput'),
				outputs: devices.filter((d) => d.kind === 'audiooutput')
			}));
		} catch (err) {
			log.error('Failed to enumerate devices', err);
		}
	}

	// Listen for hot-plug events
	if (typeof navigator !== 'undefined' && navigator.mediaDevices) {
		navigator.mediaDevices.addEventListener('devicechange', () => {
			log.info('Device change detected');
			refresh();
		});
	}

	return {
		subscribe,

		/** Refresh the device list (call after getUserMedia for labels). */
		refresh,

		/** Select a specific microphone by deviceId. Pass null for default. */
		selectMic(deviceId: string | null) {
			if (deviceId) {
				storage.set(STORAGE_KEY, deviceId);
			} else {
				storage.remove(STORAGE_KEY);
			}
			update((s) => ({ ...s, selectedMicId: deviceId }));
		},

		/** Get mic constraints with the selected device (or default). */
		getMicConstraints(purpose: MicPurpose = 'voice'): MediaStreamConstraints {
			let selectedMicId: string | null = null;
			const unsub = subscribe((s) => {
				selectedMicId = s.selectedMicId;
			});
			unsub();

			const audio: MediaTrackConstraints =
				purpose === 'clip'
					? {
							channelCount: 1,
							echoCancellation: false,
							noiseSuppression: true,
							autoGainControl: true
						}
					: {
							sampleRate: { ideal: 16000 },
							channelCount: 1,
							echoCancellation: true,
							noiseSuppression: true,
							autoGainControl: true
						};
			if (selectedMicId) {
				audio.deviceId = { exact: selectedMicId };
			}
			return { audio };
		},

		/** Acquire a mic stream, falling back to default if selected device is gone. */
		async acquireMicStream(purpose: MicPurpose = 'voice'): Promise<MediaStream> {
			try {
				const stream = await navigator.mediaDevices.getUserMedia(this.getMicConstraints(purpose));
				// Refresh device list (now we have permission, labels are populated)
				await refresh();
				return stream;
			} catch (e) {
				if (e instanceof OverconstrainedError) {
					log.warn('Selected mic unavailable, falling back to default');
					this.selectMic(null);
					const stream = await navigator.mediaDevices.getUserMedia(
						this.getMicConstraints(purpose)
					);
					await refresh();
					return stream;
				}
				throw e;
			}
		}
	};
}

export const deviceManager = createDeviceManager();
export const micDevices = derived(deviceManager, ($d) => $d.inputs);
export const selectedMicId = derived(deviceManager, ($d) => $d.selectedMicId);
