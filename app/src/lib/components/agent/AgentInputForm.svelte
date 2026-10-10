<script lang="ts">
	import type { AgentInputField } from '$lib/types/agentPage';
	import { pickFolder, pickFiles } from '$lib/api/pick';
	import { FolderOpen, FileText, Minus, Plus } from 'lucide-svelte';
	import { t } from 'svelte-i18n';
	import {
		controlFor,
		shortLabel,
		hintFor,
		fullHelp,
		unitFor,
		stepNumber,
		type FieldError
	} from './inputFields';

	// The setup questions, in one card: a short label, the full question as a
	// muted hint, and the control that fits the field's type. Rules for labels,
	// units, defaults and validation live in ./inputFields.
	let {
		fields,
		values = $bindable({}),
		errors = {},
		onchange
	}: {
		fields: AgentInputField[];
		values: Record<string, unknown>;
		/** Per-field errors to show under the control (from validateInputs). */
		errors?: Record<string, FieldError>;
		onchange?: (values: Record<string, unknown>) => void;
	} = $props();

	function handleChange(key: string, value: unknown) {
		values = { ...values, [key]: value };
		onchange?.(values);
	}

	function getStringValue(key: string): string {
		const v = values[key];
		return v != null ? String(v) : '';
	}

	function getBoolValue(field: AgentInputField): boolean {
		const v = values[field.key];
		if (typeof v === 'boolean') return v;
		if (v === 'true') return true;
		if (v === 'false') return false;
		return field.default === true;
	}

	/** A typed number stays a number; anything else stays as typed so the
	 *  field can say it is not one. */
	function handleNumberInput(key: string, raw: string) {
		const trimmed = raw.trim();
		if (trimmed === '') return handleChange(key, '');
		const n = Number(trimmed);
		handleChange(key, Number.isFinite(n) ? n : raw);
	}

	function placeholderFor(field: AgentInputField): string {
		if (field.placeholder) return field.placeholder;
		if (field.default != null && field.default !== '' && typeof field.default !== 'object') return String(field.default);
		return '';
	}

	async function browseFolder(key: string) {
		try {
			const res = await pickFolder();
			if (res.path) handleChange(key, res.path);
		} catch {
			// Native dialog not available
		}
	}

	async function browseFile(key: string) {
		try {
			const res = await pickFiles();
			if (res.paths?.length) handleChange(key, res.paths[0]);
		} catch {
			// Native dialog not available
		}
	}
</script>

<div class="rounded-xl border border-base-300 bg-base-100 divide-y divide-base-content/10">
	{#each fields as field (field.key)}
		{@const control = controlFor(field)}
		{@const label = shortLabel(field)}
		{@const hint = hintFor(field)}
		{@const unit = unitFor(field)}
		{@const error = errors[field.key]}
		{@const inputId = `input-${field.key}`}
		<div class="px-4 py-3.5">
			{#if control === 'toggle'}
				<div class="flex items-center justify-between gap-4">
					<div class="min-w-0">
						<div class="flex items-center gap-2">
							<label class="text-sm font-medium cursor-pointer" for={inputId}>{label}</label>
							{#if !field.required}<span class="badge badge-ghost badge-sm text-base-content/70">{$t('agentInputForm.optional')}</span>{/if}
						</div>
						{#if hint}<p class="text-xs text-base-content/70 mt-0.5 truncate" title={fullHelp(field)}>{hint}</p>{/if}
					</div>
					<input
						id={inputId}
						type="checkbox"
						class="toggle toggle-sm toggle-primary shrink-0"
						checked={getBoolValue(field)}
						onchange={(e) => handleChange(field.key, (e.target as HTMLInputElement).checked)}
					/>
				</div>
			{:else}
				<div class="flex items-center gap-2">
					<label class="text-sm font-medium" for={inputId}>{label}</label>
					{#if !field.required}<span class="badge badge-ghost badge-sm text-base-content/70">{$t('agentInputForm.optional')}</span>{/if}
				</div>
				{#if hint}<p class="text-xs text-base-content/70 mt-0.5 truncate" title={fullHelp(field)}>{hint}</p>{/if}

				<div class="mt-2">
					{#if control === 'number'}
						<div class="flex items-center gap-2">
							<div class="join">
								<button
									type="button"
									class="btn btn-sm btn-square join-item bg-base-200 {error ? 'border-error' : 'border-base-content/20'}"
									aria-label={$t('agentInputForm.decrease')}
									onclick={() => handleChange(field.key, stepNumber(field, values[field.key], -1))}
								>
									<Minus class="w-3.5 h-3.5" />
								</button>
								<input
									id={inputId}
									type="text"
									inputmode="decimal"
									class="input input-sm input-bordered join-item w-20 text-center focus:outline-none {error ? 'input-error' : 'border-base-content/20'}"
									placeholder={placeholderFor(field)}
									value={getStringValue(field.key)}
									aria-invalid={error ? 'true' : undefined}
									oninput={(e) => handleNumberInput(field.key, (e.target as HTMLInputElement).value)}
								/>
								<button
									type="button"
									class="btn btn-sm btn-square join-item bg-base-200 {error ? 'border-error' : 'border-base-content/20'}"
									aria-label={$t('agentInputForm.increase')}
									onclick={() => handleChange(field.key, stepNumber(field, values[field.key], 1))}
								>
									<Plus class="w-3.5 h-3.5" />
								</button>
							</div>
							{#if unit}
								<span class="text-sm text-base-content/70">{'key' in unit ? $t(`agentInputForm.${unit.key}`) : unit.text}</span>
							{/if}
						</div>

					{:else if control === 'select'}
						<select
							id={inputId}
							class="select select-sm select-bordered w-full text-sm {error ? 'select-error' : ''}"
							value={getStringValue(field.key)}
							onchange={(e) => handleChange(field.key, (e.target as HTMLSelectElement).value)}
						>
							<option value="" disabled>{$t('common.select')}</option>
							{#each field.options || [] as opt}
								<option value={opt.value}>{opt.label}</option>
							{/each}
						</select>

					{:else if control === 'textarea'}
						<textarea
							id={inputId}
							class="textarea textarea-bordered w-full text-sm {error ? 'textarea-error' : ''}"
							rows="3"
							placeholder={placeholderFor(field)}
							value={getStringValue(field.key)}
							oninput={(e) => handleChange(field.key, (e.target as HTMLTextAreaElement).value)}
						></textarea>

					{:else if control === 'path' || control === 'file'}
						<div class="flex items-center gap-2">
							<input
								id={inputId}
								type="text"
								class="input input-sm input-bordered flex-1 text-sm font-mono {error ? 'input-error' : ''}"
								placeholder={field.placeholder || (control === 'path' ? '/path/to/directory' : '/path/to/file')}
								value={getStringValue(field.key)}
								oninput={(e) => handleChange(field.key, (e.target as HTMLInputElement).value)}
							/>
							<button
								type="button"
								class="btn btn-sm btn-ghost btn-square text-primary"
								onclick={() => (control === 'path' ? browseFolder(field.key) : browseFile(field.key))}
								title={control === 'path' ? $t('agent.browseFolders') : $t('agent.browseFiles')}
							>
								{#if control === 'path'}<FolderOpen class="w-4 h-4" />{:else}<FileText class="w-4 h-4" />{/if}
							</button>
						</div>

					{:else}
						<input
							id={inputId}
							type="text"
							class="input input-sm input-bordered w-full text-sm {error ? 'input-error' : ''}"
							placeholder={placeholderFor(field)}
							value={getStringValue(field.key)}
							oninput={(e) => handleChange(field.key, (e.target as HTMLInputElement).value)}
						/>
					{/if}
				</div>
			{/if}
			{#if error}
				<p class="text-xs text-error mt-1.5" role="alert">{$t(error.key, { values: error.values })}</p>
			{/if}
		</div>
	{/each}
</div>
