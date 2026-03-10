<script lang="ts">
	export interface AskWidgetDef {
		type: 'buttons' | 'select' | 'confirm' | 'radio' | 'checkbox';
		label?: string;
		options?: string[];
		default?: string;
	}

	interface Props {
		requestId: string;
		prompt: string;
		widgets: AskWidgetDef[];
		response?: string;
		disabled?: boolean;
		onSubmit: (requestId: string, value: string) => void;
	}

	let { requestId, prompt, widgets, response, disabled = false, onSubmit }: Props = $props();

	let selectValue = $state('');
	let radioValue = $state('');
	let selectedOptions = $state(new Set<string>());

	const answered = $derived(response != null && response !== undefined);

	function submit(value: string) {
		if (!answered) {
			onSubmit(requestId, value);
		}
	}

	function toggleOption(option: string) {
		const next = new Set(selectedOptions);
		if (next.has(option)) {
			next.delete(option);
		} else {
			next.add(option);
		}
		selectedOptions = next;
	}
</script>

<div class="rounded-xl bg-base-200 px-4 py-3 mb-1 max-w-md">
	<p class="text-sm font-medium mb-2">{prompt}</p>

	{#if answered}
		<div class="flex flex-wrap gap-1">
			{#each (response ?? '').split(', ') as item}
				<div class="badge badge-primary badge-sm">{item}</div>
			{/each}
		</div>
	{:else if disabled}
		<div class="flex flex-wrap gap-1">
			<div class="badge badge-ghost badge-sm">Skipped</div>
		</div>
	{:else}
		{#each widgets as widget}
			{#if widget.label}
				<p class="text-xs text-base-content/60 mb-1">{widget.label}</p>
			{/if}

			{#if widget.type === 'buttons' || widget.type === 'confirm'}
				<div class="flex flex-wrap gap-2">
					{#each widget.options ?? ['Yes', 'No'] as option}
						<button
							type="button"
							class="btn btn-sm btn-outline"
							onclick={() => submit(option)}
						>
							{option}
						</button>
					{/each}
				</div>
			{:else if widget.type === 'select'}
				<div class="flex gap-2 items-center">
					<select
						class="select select-bordered select-sm flex-1"
						bind:value={selectValue}
					>
						<option value="" disabled selected>Choose...</option>
						{#each widget.options ?? [] as option}
							<option value={option}>{option}</option>
						{/each}
					</select>
					<button
						type="button"
						class="btn btn-sm btn-primary"
						disabled={!selectValue}
						onclick={() => submit(selectValue)}
					>
						OK
					</button>
				</div>
			{:else if widget.type === 'radio'}
				<div class="flex flex-col gap-1">
					{#each widget.options ?? [] as option}
						<label class="label cursor-pointer justify-start gap-2">
							<input
								type="radio"
								name="ask-radio-{requestId}"
								class="radio radio-sm radio-primary"
								value={option}
								bind:group={radioValue}
							/>
							<span class="label-text">{option}</span>
						</label>
					{/each}
					<button
						type="button"
						class="btn btn-sm btn-primary mt-1 self-start"
						disabled={!radioValue}
						onclick={() => submit(radioValue)}
					>
						Submit
					</button>
				</div>
			{:else if widget.type === 'checkbox'}
				<div class="flex flex-col gap-1">
					{#each widget.options ?? [] as option}
						<label class="label cursor-pointer justify-start gap-2">
							<input
								type="checkbox"
								class="checkbox checkbox-sm checkbox-primary"
								checked={selectedOptions.has(option)}
								onchange={() => toggleOption(option)}
							/>
							<span class="label-text">{option}</span>
						</label>
					{/each}
					<button
						type="button"
						class="btn btn-sm btn-primary mt-1 self-start"
						disabled={selectedOptions.size === 0}
						onclick={() => submit([...selectedOptions].join(', '))}
					>
						Submit ({selectedOptions.size})
					</button>
				</div>
			{/if}
		{/each}
	{/if}
</div>
