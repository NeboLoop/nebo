/**
 * Setup Wizard State Store
 *
 * Manages the state of the setup wizard using Svelte 5 runes.
 * Persists to localStorage so users can resume setup.
 */

const STORAGE_KEY = 'gobot-setup';

/**
 * Setup state interface
 */
export interface SetupState {
	mode: 'quickstart' | 'advanced';
	currentStep: number;
	securityAcknowledged: boolean;
	accountCreated: boolean;
	providerConfigured: boolean;
	completed: boolean;
}

/**
 * Step names for each mode
 */
const QUICKSTART_STEPS = ['welcome', 'account', 'provider', 'complete'] as const;
const ADVANCED_STEPS = [
	'welcome',
	'account',
	'provider',
	'models',
	'permissions',
	'personality',
	'complete'
] as const;

type QuickstartStep = (typeof QUICKSTART_STEPS)[number];
type AdvancedStep = (typeof ADVANCED_STEPS)[number];
type StepName = QuickstartStep | AdvancedStep;

/**
 * Initial state
 */
const initialState: SetupState = {
	mode: 'quickstart',
	currentStep: 0,
	securityAcknowledged: false,
	accountCreated: false,
	providerConfigured: false,
	completed: false
};

/**
 * Load state from localStorage
 */
function loadState(): SetupState {
	if (typeof window === 'undefined') {
		return initialState;
	}

	try {
		const stored = localStorage.getItem(STORAGE_KEY);
		if (stored) {
			const parsed = JSON.parse(stored) as Partial<SetupState>;
			return { ...initialState, ...parsed };
		}
	} catch {
		// Ignore parse errors, use initial state
	}

	return initialState;
}

/**
 * Save state to localStorage
 */
function saveState(state: SetupState): void {
	if (typeof window === 'undefined') return;

	try {
		localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
	} catch {
		// Ignore storage errors
	}
}

/**
 * Create the setup store using Svelte 5 runes
 */
function createSetupStore() {
	let state = $state<SetupState>(loadState());

	// Derived: total steps based on mode
	const totalSteps = $derived(state.mode === 'quickstart' ? QUICKSTART_STEPS.length : ADVANCED_STEPS.length);

	// Derived: current step name
	const stepName = $derived(
		(state.mode === 'quickstart' ? QUICKSTART_STEPS : ADVANCED_STEPS)[state.currentStep] ?? 'welcome'
	);

	/**
	 * Persist state changes
	 */
	function persist() {
		saveState(state);
	}

	/**
	 * Navigate to next step
	 */
	function nextStep(): void {
		const maxStep = totalSteps - 1;
		if (state.currentStep < maxStep) {
			state.currentStep++;
			persist();
		}
	}

	/**
	 * Navigate to previous step
	 */
	function prevStep(): void {
		if (state.currentStep > 0) {
			state.currentStep--;
			persist();
		}
	}

	/**
	 * Set the wizard mode
	 */
	function setMode(mode: 'quickstart' | 'advanced'): void {
		state.mode = mode;
		// Reset step when changing mode to avoid out-of-bounds
		if (mode === 'quickstart' && state.currentStep >= QUICKSTART_STEPS.length) {
			state.currentStep = 0;
		}
		persist();
	}

	/**
	 * Acknowledge security notice
	 */
	function acknowledgeSecruity(): void {
		state.securityAcknowledged = true;
		persist();
	}

	/**
	 * Mark account as created
	 */
	function markAccountCreated(): void {
		state.accountCreated = true;
		persist();
	}

	/**
	 * Mark provider as configured
	 */
	function markProviderConfigured(): void {
		state.providerConfigured = true;
		persist();
	}

	/**
	 * Mark setup as complete
	 */
	function markComplete(): void {
		state.completed = true;
		persist();
	}

	/**
	 * Reset the wizard state
	 */
	function reset(): void {
		state.mode = initialState.mode;
		state.currentStep = initialState.currentStep;
		state.securityAcknowledged = initialState.securityAcknowledged;
		state.accountCreated = initialState.accountCreated;
		state.providerConfigured = initialState.providerConfigured;
		state.completed = initialState.completed;
		persist();
	}

	/**
	 * Go to a specific step
	 */
	function goToStep(step: number): void {
		const maxStep = totalSteps - 1;
		if (step >= 0 && step <= maxStep) {
			state.currentStep = step;
			persist();
		}
	}

	return {
		// Expose state as getter for reactivity
		get state() {
			return state;
		},
		get totalSteps() {
			return totalSteps;
		},
		get stepName() {
			const steps = state.mode === 'quickstart' ? QUICKSTART_STEPS : ADVANCED_STEPS;
			return steps[state.currentStep] ?? 'welcome';
		},
		// Actions
		nextStep,
		prevStep,
		setMode,
		acknowledgeSecruity,
		markAccountCreated,
		markProviderConfigured,
		markComplete,
		reset,
		goToStep
	};
}

// Export singleton instance
export const setup = createSetupStore();

// Export convenience accessors
export const setupState = {
	get mode() {
		return setup.state.mode;
	},
	get currentStep() {
		return setup.state.currentStep;
	},
	get securityAcknowledged() {
		return setup.state.securityAcknowledged;
	},
	get accountCreated() {
		return setup.state.accountCreated;
	},
	get providerConfigured() {
		return setup.state.providerConfigured;
	},
	get completed() {
		return setup.state.completed;
	},
	get totalSteps() {
		return setup.totalSteps;
	},
	get stepName() {
		return setup.stepName;
	}
};
