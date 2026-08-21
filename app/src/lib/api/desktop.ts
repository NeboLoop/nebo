// Teach-a-task recording control for the bot's computer (cloud desktop
// session). Not yet in the generated `nebo.ts`, so it lives here against the
// same `webapi` request layer.

import { webapi } from './gocliRequest';

export interface TeachStartResponse {
	sessionId: string;
	dir: string;
}

export interface TeachStopResponse {
	sessionId: string;
	dir: string;
	keyframes: number;
}

/** POST /desktop/teach/start — start recording a demonstration (starts the desktop if needed). */
export function teachStart() {
	return webapi.post<TeachStartResponse>('/api/v1/desktop/teach/start', {});
}

/** POST /desktop/teach/stop — finalize the recording and return its artifacts. */
export function teachStop() {
	return webapi.post<TeachStopResponse>('/api/v1/desktop/teach/stop', {});
}
