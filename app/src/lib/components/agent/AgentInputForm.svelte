<script lang="ts">
	import type { AgentInputField } from '$lib/api/neboComponents';
	import { pickFolder, pickFiles } from '$lib/api/nebo';
	import { FolderOpen, FileText } from 'lucide-svelte';

	let {
		fields,
		values = $bindable({}),
		onchange,
	}: {
		fields: AgentInputField[];
		values: Record<string, unknown>;
		onchange?: (values: Record<string, unknown>) => void;
	} = $props();

	function handleChange(key: string, value: unknown) {
		values = { ...values, [key]: value };
		onchange?.(values);
	}

	function getStringValue(key: string, fallback: string = ''): string {
		const v = values[key];
		return v != null ? String(v) : fallback;
	}

	function getNumberValue(key: string, fallback: number = 0): number {
		const v = values[key];
		return typeof v === 'number' ? v : (v != null ? Number(v) || fallback : fallback);
	}

	function getBoolValue(key: string, fallback: boolean = false): boolean {
		const v = values[key];
		return typeof v === 'boolean' ? v : fallback;
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

<div class="flex flex-col gap-4">
	{#each fields as field}
		<div>
			<label class="block text-sm font-medium mb-1" for="input-{field.key}">
				{field.label}
				{#if field.required}
					<span class="text-error">*</span>
				{/if}
			</label>
			{#if field.description}
				<p class="text-xs text-base-content/70 mb-1.5">{field.description}</p>
			{/if}

			{#if field.type === 'path'}
				<!-- Directory picker -->
				<div class="flex items-center gap-2">
					<input
						id="input-{field.key}"
						type="text"
						class="input input-bordered flex-1 text-sm font-mono"
						placeholder={field.placeholder || '/path/to/directory'}
						value={getStringValue(field.key, field.default != null ? String(field.default) : '')}
						oninput={(e) => handleChange(field.key, (e.target as HTMLInputElement).value)}
					/>
					<button
						type="button"
						class="btn btn-sm btn-ghost btn-square text-primary"
						onclick={() => browseFolder(field.key)}
						title="Browse folders"
					>
						<FolderOpen class="w-4 h-4" />
					</button>
				</div>

			{:else if field.type === 'file'}
				<!-- File picker -->
				<div class="flex items-center gap-2">
					<input
						id="input-{field.key}"
						type="text"
						class="input input-bordered flex-1 text-sm font-mono"
						placeholder={field.placeholder || '/path/to/file'}
						value={getStringValue(field.key, field.default != null ? String(field.default) : '')}
						oninput={(e) => handleChange(field.key, (e.target as HTMLInputElement).value)}
					/>
					<button
						type="button"
						class="btn btn-sm btn-ghost btn-square text-primary"
						onclick={() => browseFile(field.key)}
						title="Browse files"
					>
						<FileText class="w-4 h-4" />
					</button>
				</div>

			{:else if field.type === 'textarea'}
				<textarea
					id="input-{field.key}"
					class="textarea textarea-bordered w-full text-sm"
					rows="3"
					placeholder={field.placeholder || ''}
					value={getStringValue(field.key, field.default != null ? String(field.default) : '')}
					oninput={(e) => handleChange(field.key, (e.target as HTMLTextAreaElement).value)}
				></textarea>

			{:else if field.type === 'number'}
				<input
					id="input-{field.key}"
					type="number"
					class="input input-bordered w-full max-w-xs text-sm"
					placeholder={field.placeholder || ''}
					value={getNumberValue(field.key, typeof field.default === 'number' ? field.default : 0)}
					oninput={(e) => handleChange(field.key, Number((e.target as HTMLInputElement).value))}
				/>

			{:else if field.type === 'select'}
				<select
					id="input-{field.key}"
					class="select select-bordered w-full max-w-xs text-sm"
					value={getStringValue(field.key, field.default != null ? String(field.default) : '')}
					onchange={(e) => handleChange(field.key, (e.target as HTMLSelectElement).value)}
				>
					<option value="" disabled>Select...</option>
					{#each field.options || [] as opt}
						<option value={opt.value}>{opt.label}</option>
					{/each}
				</select>

			{:else if field.type === 'checkbox'}
				<label class="flex items-center gap-2 cursor-pointer">
					<input
						id="input-{field.key}"
						type="checkbox"
						class="checkbox checkbox-sm checkbox-primary"
						checked={getBoolValue(field.key, field.default === true)}
						onchange={(e) => handleChange(field.key, (e.target as HTMLInputElement).checked)}
					/>
					<span class="text-sm text-base-content/70">{field.label}</span>
				</label>

			{:else if field.type === 'radio'}
				<div class="flex flex-col gap-1.5">
					{#each field.options || [] as opt}
						<label class="flex items-center gap-2 cursor-pointer">
							<input
								type="radio"
								name="input-{field.key}"
								class="radio radio-sm radio-primary"
								value={opt.value}
								checked={getStringValue(field.key, field.default != null ? String(field.default) : '') === opt.value}
								onchange={() => handleChange(field.key, opt.value)}
							/>
							<span class="text-sm text-base-content/70">{opt.label}</span>
						</label>
					{/each}
				</div>

			{:else}
				<!-- text (default) -->
				<input
					id="input-{field.key}"
					type="text"
					class="input input-bordered w-full max-w-md text-sm"
					placeholder={field.placeholder || ''}
					value={getStringValue(field.key, field.default != null ? String(field.default) : '')}
					oninput={(e) => handleChange(field.key, (e.target as HTMLInputElement).value)}
				/>
			{/if}
		</div>
	{/each}
</div>
