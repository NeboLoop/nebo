// Migration importer endpoints (Settings → Import, onboarding detect card).
//
// The generated `nebo.ts` client does not yet cover them, so they live here
// against the same `webapi` request layer (auth header + base URL handling
// included), like `pluginAccounts.ts`.

import { webapi } from './gocliRequest';

export interface DetectedInstall {
	source: 'hermes' | 'openclaw';
	path: string;
	/** False while a system's apply path hasn't shipped (OpenClaw), so the UI
	 * shows it as "coming soon" instead of offering a broken import. */
	importable: boolean;
}

export interface DetectInstallsResponse {
	installs: DetectedInstall[];
}

export interface ImportItem {
	kind: 'mcp_server' | 'skill' | 'agent' | 'memory' | 'session' | 'cron' | 'credential';
	/** content = adopt silently; code = runs on this machine; reference = remote payload. */
	tier: 'content' | 'code' | 'reference';
	name: string;
	detail: string;
	target: string;
	sourcePath: string;
}

export interface ImportManifest {
	source: 'hermes' | 'openclaw';
	root: string;
	items: ImportItem[];
	notes: string[];
}

export interface ScanInstallResponse {
	manifest: ImportManifest;
	needsConfirmation: boolean;
}

export interface ImportOutcome {
	agents: number;
	skills: number;
	mcpServers: number;
	authProfiles: number;
	agentId: string | null;
	agentName: string | null;
	skipped: string[];
}

export interface ApplyInstallResponse {
	outcome: ImportOutcome;
}

/** GET /import/detect — probe default install locations (fingerprinted). */
export function detectInstalls() {
	return webapi.get<DetectInstallsResponse>(`/api/v1/import/detect`);
}

/** POST /import/scan — dry-run: what an import of `path` would do. */
export function scanInstall(path: string) {
	return webapi.post<ScanInstallResponse>(`/api/v1/import/scan`, { path });
}

/** POST /import/apply — perform the import of `path`. Call only after the
 * user approved the scanned manifest. */
export function applyInstall(path: string) {
	return webapi.post<ApplyInstallResponse>(`/api/v1/import/apply`, { path });
}
