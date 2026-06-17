import { writable } from 'svelte/store';

/**
 * Single source of truth for the install / configure modal.
 *
 * There is exactly ONE `<InstallFlowModal>` mounted (in the root layout). Every
 * entry point opens it through this store — marketplace Install, ProductDetail
 * Configure, an agent's Settings → Configure (`open`), and a pasted install code
 * (`openCode`) — instead of mounting its own copy (which used to stack/duplicate
 * modals) or firing a `window` CustomEvent (a dead second pathway). Once open,
 * progress is driven by the backend's `code_*` / `dep_*` WS events, which the
 * modal subscribes to directly via the WS emitter.
 */

export type InstallMode = 'code' | 'product' | 'configure';

/** Optimistic open payload for a pasted install code — opens the modal before the
 *  backend's `code_processing` WS frame arrives, closing the submit→round-trip gap. */
export interface CodeOpenOptions {
	code: string;
	codeType: string;
	statusMessage: string;
	/** Desktop-initiated paste: the modal stays open until the user dismisses it. */
	interactive: boolean;
}

export interface InstallFlowOptions {
	/** 'product' = install from marketplace; 'configure' = edit an installed agent. */
	mode: Exclude<InstallMode, 'code'>;
	/** Marketplace product id (product mode). */
	appId?: string;
	/** Installed agent id (configure mode). */
	existingAgentId?: string;
	agentName?: string;
	agentDescription?: string;
	/** agent.json input field definitions/defaults to seed the form. */
	seedInputs?: Record<string, unknown> | Record<string, unknown>[];
	/** Declared dependency object so dep rows render before the cascade reports. */
	dependencies?: unknown;
	/** Called when the flow finishes (e.g. navigate to the agent). */
	oncomplete?: (agentId?: string) => void;
	/** Configure mode: invoked when the user clicks Uninstall. */
	onUninstall?: () => void;
}

export interface InstallFlowState
	extends Partial<Omit<InstallFlowOptions, 'mode'>>,
		Partial<CodeOpenOptions> {
	show: boolean;
	mode: InstallMode;
}

const CLOSED: InstallFlowState = { show: false, mode: 'code' };

function createInstallFlow() {
	const { subscribe, set } = writable<InstallFlowState>({ ...CLOSED });
	return {
		subscribe,
		/** Open the shared modal for a product install or agent configure. */
		open(opts: InstallFlowOptions) {
			set({ ...opts, show: true });
		},
		/** Open the shared modal optimistically for a pasted install code. */
		openCode(opts: CodeOpenOptions) {
			set({ ...opts, mode: 'code', show: true });
		},
		/** Close + clear. The modal calls this on dismiss. */
		close() {
			set({ ...CLOSED });
		},
	};
}

export const installFlow = createInstallFlow();
