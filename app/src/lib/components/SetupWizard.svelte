<script lang="ts">
	// Multi-step setup wizard. Driven by an ArtifactSetup config from the
	// artifact's manifest (plugin.json / SKILL.md / agent.json) — same
	// schema across every artifact type. Each step has a `kind`:
	//   - form        → collect values for later substitution
	//   - generate    → run plugin command, show stdout in a code block
	//   - external    → open an external URL with instructions
	//   - credentials → collect secrets, save as plugin env vars
	//
	// The wizard owns its own step index, form state, and generated
	// output cache. Callers provide `slug`, `setup`, and a callback for
	// when the user completes the credentials save.

	import { pluginSetupRun } from '$lib/api/nebo';

	interface SetupField {
		key: string;
		label: string;
		default?: string;
		placeholder?: string;
		maxLength?: number;
		help?: string;
		inputType?: 'text' | 'textarea' | 'password';
		required?: boolean;
	}

	type SetupStep =
		| { kind: 'form'; title: string; description?: string; fields: SetupField[] }
		| {
				kind: 'generate';
				title: string;
				description?: string;
				command: string;
				args: string[];
				outputFormat?: string;
				buttonLabel?: string;
		  }
		| {
				kind: 'external';
				title: string;
				description?: string;
				url: string;
				urlLabel: string;
				instructions?: string[];
		  }
		| {
				kind: 'credentials';
				title: string;
				description?: string;
				fields: SetupField[];
				verifyCommand?: string;
		  };

	interface ArtifactSetup {
		title?: string;
		description?: string;
		steps: SetupStep[];
	}

	type Props = {
		slug: string;
		setup: ArtifactSetup;
		onClose: () => void;
		onComplete: (envValues: Record<string, string>) => void | Promise<void>;
		// Previously-saved non-secret values (e.g. bot name) to pre-fill so a
		// re-run remembers what was set. Takes priority over field defaults.
		initialValues?: Record<string, string>;
		// Keys already stored server-side (e.g. secret tokens we can't show).
		// Fields with these keys render as "already set — leave blank to keep"
		// and don't block the step when empty.
		alreadySetKeys?: string[];
	};

	let { slug, setup, onClose, onComplete, initialValues = {}, alreadySetKeys = [] }: Props = $props();

	let stepIndex = $state(0);
	// Form values across the whole wizard — used both for {{key}} substitution
	// in generate steps and as env vars in the credentials step. Seeded from
	// any previously-saved values so a re-run remembers them.
	let values: Record<string, string> = $state({ ...initialValues });

	const alreadySet = (key: string) => alreadySetKeys.includes(key);
	let generatedOutputs: Record<number, string> = $state({});
	let generatedFormats: Record<number, string> = $state({});
	let runningGenerate = $state(false);
	let generateError = $state<string | null>(null);
	let copyState = $state<'idle' | 'copied'>('idle');
	let savingCredentials = $state(false);
	let credentialsError = $state<string | null>(null);

	const currentStep = $derived(setup.steps[stepIndex]);
	const isLastStep = $derived(stepIndex === setup.steps.length - 1);

	// Pre-seed defaults the first time we land on a form/credentials step.
	$effect(() => {
		if (
			currentStep &&
			(currentStep.kind === 'form' || currentStep.kind === 'credentials')
		) {
			for (const field of currentStep.fields) {
				if (values[field.key] === undefined && field.default !== undefined) {
					values[field.key] = field.default;
				}
			}
		}
	});

	function stepIsValid(): boolean {
		if (!currentStep) return false;
		if (currentStep.kind === 'form' || currentStep.kind === 'credentials') {
			return currentStep.fields.every(
				(f) =>
					!f.required ||
					(values[f.key] ?? '').trim().length > 0 ||
					alreadySet(f.key) // already stored server-side; blank = keep existing
			);
		}
		if (currentStep.kind === 'generate') {
			// Generate steps require the command to have been run.
			return generatedOutputs[stepIndex] !== undefined;
		}
		return true;
	}

	async function runGenerate() {
		if (currentStep?.kind !== 'generate') return;
		runningGenerate = true;
		generateError = null;
		try {
			const data = await pluginSetupRun(slug, { stepIndex, values });
			if (data.ok === false) {
				const d = data as { error?: string; stderr?: string };
				generateError =
					d.error || d.stderr || 'Setup step failed. Check that the plugin is installed.';
				return;
			}
			generatedOutputs[stepIndex] = (data.output as string) ?? '';
			generatedFormats[stepIndex] = (data.outputFormat as string) ?? currentStep.outputFormat ?? 'text';
		} catch (err) {
			generateError = (err as Error).message || 'Request failed.';
		} finally {
			runningGenerate = false;
		}
	}

	async function copyGenerated() {
		const out = generatedOutputs[stepIndex];
		if (!out) return;
		try {
			await navigator.clipboard.writeText(out);
			copyState = 'copied';
			setTimeout(() => (copyState = 'idle'), 1500);
		} catch {
			// Fallback: select-and-execCommand path if clipboard API is locked down.
		}
	}

	function openExternal(url: string) {
		window.open(url, '_blank', 'noopener,noreferrer');
	}

	async function saveCredentials() {
		if (currentStep?.kind !== 'credentials') return;
		savingCredentials = true;
		credentialsError = null;
		try {
			// Persist ALL collected values (form fields like name/description
			// from earlier steps + this step's credentials) so a re-run
			// remembers them. Blank fields already stored server-side are left
			// untouched — don't wipe an existing token with an empty string.
			const envValues: Record<string, string> = {};
			for (const [key, raw] of Object.entries(values)) {
				const v = (raw ?? '').trim();
				if (v.length > 0) {
					envValues[key] = v;
				} else if (!alreadySet(key)) {
					envValues[key] = '';
				}
			}
			await onComplete(envValues);
		} catch (err) {
			credentialsError = (err as Error).message || 'Save failed.';
		} finally {
			savingCredentials = false;
		}
	}

	function next() {
		if (!stepIsValid()) return;
		if (isLastStep) return;
		stepIndex += 1;
	}

	function back() {
		if (stepIndex === 0) return;
		stepIndex -= 1;
	}
</script>

<div class="modal modal-open">
	<div class="modal-box max-w-2xl bg-base-100 border border-base-300">
		<div class="flex items-start justify-between gap-4 mb-1">
			<div>
				{#if setup.title}
					<div class="text-base font-semibold">{setup.title}</div>
				{/if}
				{#if setup.description}
					<div class="text-xs text-base-content/70 mt-1">{setup.description}</div>
				{/if}
			</div>
			<button class="btn btn-sm btn-ghost btn-square" onclick={onClose} aria-label="Close">
				<span aria-hidden="true">×</span>
			</button>
		</div>

		<!-- Step progress -->
		<div class="flex items-center gap-2 mt-4 mb-5">
			{#each setup.steps as _, idx}
				<div
					class="h-1 flex-1 rounded {idx <= stepIndex
						? 'bg-primary'
						: 'bg-base-300'}"
					aria-hidden="true"
				></div>
			{/each}
		</div>
		<div class="text-xs font-mono text-base-content/50 mb-2">
			Step {stepIndex + 1} of {setup.steps.length}
		</div>

		{#if currentStep}
			<div class="text-sm font-medium mb-1">{currentStep.title}</div>
			{#if 'description' in currentStep && currentStep.description}
				<div class="text-xs text-base-content/70 mb-4">{currentStep.description}</div>
			{/if}

			{#if currentStep.kind === 'form' || currentStep.kind === 'credentials'}
				<div class="flex flex-col gap-3">
					{#each currentStep.fields as field}
						<label class="form-control w-full">
							<div class="label py-1">
								<span class="label-text text-sm">{field.label}</span>
								{#if field.maxLength}
									<span class="label-text-alt text-xs text-base-content/50 font-mono">
										{(values[field.key] ?? '').length} / {field.maxLength}
									</span>
								{/if}
							</div>
							{#if field.inputType === 'textarea'}
								<textarea
									class="textarea textarea-bordered w-full text-sm"
									placeholder={field.placeholder ?? ''}
									maxlength={field.maxLength ?? undefined}
									bind:value={values[field.key]}
								></textarea>
							{:else}
								<input
									type={field.inputType === 'password' ? 'password' : 'text'}
									class="input input-bordered w-full text-sm {field.inputType === 'password'
										? 'font-mono'
										: ''}"
									placeholder={alreadySet(field.key) && !(values[field.key] ?? '').length
										? '•••••••• already set — leave blank to keep'
										: (field.placeholder ?? '')}
									maxlength={field.maxLength ?? undefined}
									bind:value={values[field.key]}
								/>
							{/if}
							{#if field.help}
								<div class="label py-1">
									<span class="label-text-alt text-xs text-base-content/50">{field.help}</span>
								</div>
							{/if}
						</label>
					{/each}
				</div>

				{#if currentStep.kind === 'credentials' && credentialsError}
					<div class="alert alert-error mt-4 text-sm">
						<span>{credentialsError}</span>
					</div>
				{/if}
			{:else if currentStep.kind === 'generate'}
				{#if !generatedOutputs[stepIndex]}
					<button
						class="btn btn-primary btn-sm"
						onclick={runGenerate}
						disabled={runningGenerate}
					>
						{runningGenerate ? 'Generating…' : currentStep.buttonLabel || 'Generate'}
					</button>
					{#if generateError}
						<div class="alert alert-error mt-4 text-sm">
							<span>{generateError}</span>
						</div>
					{/if}
				{:else}
					<div class="relative">
						<div class="absolute top-2 right-2 z-10">
							<button class="btn btn-xs btn-ghost" onclick={copyGenerated}>
								{copyState === 'copied' ? 'Copied ✓' : 'Copy'}
							</button>
						</div>
						<pre
							class="bg-base-200 border border-base-300 rounded p-4 text-xs font-mono overflow-auto max-h-80 whitespace-pre"><code>{generatedOutputs[stepIndex]}</code></pre>
					</div>
					<button class="btn btn-ghost btn-xs mt-2" onclick={runGenerate} disabled={runningGenerate}>
						Regenerate
					</button>
				{/if}
			{:else if currentStep.kind === 'external'}
				<button
					class="btn btn-primary btn-sm gap-2"
					onclick={() => openExternal(currentStep.url)}
				>
					{currentStep.urlLabel}
					<span aria-hidden="true">↗</span>
				</button>
				{#if currentStep.instructions && currentStep.instructions.length > 0}
					<ol class="mt-4 space-y-2 list-decimal list-inside text-sm">
						{#each currentStep.instructions as inst}
							<li class="text-base-content/80">{inst}</li>
						{/each}
					</ol>
				{/if}
			{/if}
		{/if}

		<!-- Footer / nav -->
		<div class="modal-action mt-6">
			<button class="btn btn-ghost btn-sm" onclick={back} disabled={stepIndex === 0}>
				Back
			</button>
			{#if currentStep?.kind === 'credentials'}
				<button
					class="btn btn-primary btn-sm"
					disabled={!stepIsValid() || savingCredentials}
					onclick={saveCredentials}
				>
					{savingCredentials ? 'Saving…' : 'Save & Verify'}
				</button>
			{:else if !isLastStep}
				<button
					class="btn btn-primary btn-sm"
					disabled={!stepIsValid()}
					onclick={next}
				>
					Next
				</button>
			{/if}
		</div>
	</div>
</div>
