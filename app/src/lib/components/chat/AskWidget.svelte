<script lang="ts" module>
	/** Sent as the answer when the user dismisses instead of choosing. Mirrors
	 * `SKIP_SENTINEL` in crates/tools/src/origin.rs. */
	export const SKIP_VALUE = '__skip__';

	export type AskOption = string | { label: string; description?: string; recommended?: boolean };

	export interface AskWidgetDef {
		/** 'options' is canonical; legacy single-choice shapes still render. */
		type: 'options' | 'buttons' | 'confirm' | 'select' | 'radio' | 'checkbox';
		label?: string;
		options?: AskOption[];
		multiSelect?: boolean;
		default?: string;
	}

	interface NormalizedOption {
		label: string;
		description?: string;
		recommended?: boolean;
	}

	function normalizeOptions(options: AskOption[] | undefined): NormalizedOption[] {
		return (options ?? []).map((o) =>
			typeof o === 'string' ? { label: o } : { label: o.label, description: o.description, recommended: o.recommended }
		);
	}
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';

	interface Props {
		requestId: string;
		prompt: string;
		widgets: AskWidgetDef[];
		response?: string;
		disabled?: boolean;
		onSubmit: (requestId: string, value: string) => void;
	}

	let { requestId, prompt, widgets, response, disabled = false, onSubmit }: Props = $props();

	// The prompt is agent/harness-authored text (e.g. the deep-research plan)
	// and uses markdown like every other agent message — render it, don't show
	// raw ** markers. Same marked pipeline as the chat transcript.
	import { marked } from 'marked';
	const promptHtml = $derived(marked.parse(prompt, { async: false }) as string);

	const widget = $derived(widgets?.[0]);
	const options = $derived(normalizeOptions(widget?.options));
	const isMulti = $derived(widget?.multiSelect === true || widget?.type === 'checkbox');

	let selected = $state(new Set<string>());
	let showOther = $state(false);
	let otherText = $state('');

	const answered = $derived(response != null);
	const wasSkipped = $derived(response === SKIP_VALUE);

	function submit(value: string) {
		if (!answered && !disabled) {
			onSubmit(requestId, value);
		}
	}

	function toggle(label: string) {
		const next = new Set(selected);
		if (next.has(label)) next.delete(label);
		else next.add(label);
		selected = next;
	}

	function submitOther() {
		const v = otherText.trim();
		if (v) submit(v);
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && !answered && !disabled) {
			submit(SKIP_VALUE);
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

<div class="rounded-xl bg-base-200 px-4 py-3 mb-1 max-w-md">
	<div class="text-sm font-medium mb-2 prose prose-sm max-w-none [&_p]:my-1 [&>:first-child]:mt-0 [&>:last-child]:mb-0">{@html promptHtml}</div>

	{#if answered}
		{#if wasSkipped}
			<div class="badge badge-ghost badge-sm">{$t('common.skipped')}</div>
		{:else}
			<div class="flex flex-wrap gap-1">
				{#each (response ?? '').split(', ') as item}
					<div class="badge badge-primary badge-sm">{item}</div>
				{/each}
			</div>
		{/if}
	{:else if disabled}
		<div class="badge badge-ghost badge-sm">{$t('common.skipped')}</div>
	{:else}
		{#if widget?.label}
			<p class="text-xs text-base-content/70 mb-1">{widget.label}</p>
		{/if}

		{#if isMulti}
			<div class="flex flex-col gap-1">
				{#each options as option}
					<label class="label cursor-pointer justify-start gap-2 py-1">
						<input
							type="checkbox"
							class="checkbox checkbox-sm checkbox-primary"
							checked={selected.has(option.label)}
							onchange={() => toggle(option.label)}
						/>
						<span class="flex flex-col">
							<span class="text-sm">
								{option.label}
								{#if option.recommended}<span class="badge badge-primary badge-xs ml-1">{$t('chat.recommended')}</span>{/if}
							</span>
							{#if option.description}<span class="text-xs text-base-content/70">{option.description}</span>{/if}
						</span>
					</label>
				{/each}
			</div>
		{:else}
			<div class="flex flex-col gap-1.5">
				{#each options as option}
					<button
						type="button"
						class="btn btn-sm btn-outline justify-start h-auto py-1.5 normal-case"
						onclick={() => submit(option.label)}
					>
						<span class="flex flex-col items-start text-left">
							<span class="font-medium">
								{option.label}
								{#if option.recommended}<span class="badge badge-primary badge-xs ml-1">{$t('chat.recommended')}</span>{/if}
							</span>
							{#if option.description}<span class="text-xs text-base-content/70 font-normal">{option.description}</span>{/if}
						</span>
					</button>
				{/each}
			</div>
		{/if}

		<!-- Free-text escape + dismiss -->
		<div class="mt-2 flex flex-col gap-2">
			{#if showOther}
				<div class="flex gap-2 items-center">
					<input
						type="text"
						class="input input-bordered input-sm flex-1"
						placeholder={$t('chat.typeYourAnswer')}
						bind:value={otherText}
						onkeydown={(e) => e.key === 'Enter' && submitOther()}
					/>
					<button type="button" class="btn btn-sm btn-primary" disabled={!otherText.trim()} onclick={submitOther}>{$t('common.ok')}</button>
				</div>
			{/if}

			<div class="flex items-center gap-3">
				{#if isMulti}
					<button
						type="button"
						class="btn btn-sm btn-primary"
						disabled={selected.size === 0}
						onclick={() => submit([...selected].join(', '))}
					>
						{$t('chat.submit')}{selected.size > 0 ? ` (${selected.size})` : ''}
					</button>
				{/if}
				{#if !showOther}
					<button type="button" class="text-xs text-base-content/60 hover:text-base-content cursor-pointer bg-transparent border-none px-0" onclick={() => (showOther = true)}>{$t('chat.other')}</button>
				{/if}
				<button type="button" class="text-xs text-base-content/40 hover:text-base-content/70 cursor-pointer bg-transparent border-none px-0 ml-auto" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
			</div>
		</div>
	{/if}
</div>
