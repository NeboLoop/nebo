import { writable } from 'svelte/store';

/**
 * Single source of truth for the install / configure modal.
 *
 * There is exactly ONE `<InstallFlowModal>` mounted (in the root layout). Every
 * entry point — marketplace Install, ProductDetail Configure, an agent's
 * Settings → Configure — opens it through this store instead of mounting its own
 * copy (which used to stack/duplicate modals). Code-paste installs are separate:
 * they flow through chat → backend → `nebo:code_*` window events, which the same
 * single modal listens for.
 */

export type InstallMode = 'code' | 'product' | 'configure';

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

export interface InstallFlowState extends Partial<Omit<InstallFlowOptions, 'mode'>> {
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
		/** Close + clear. The modal calls this on dismiss. */
		close() {
			set({ ...CLOSED });
		},
	};
}

export const installFlow = createInstallFlow();
