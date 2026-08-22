<script lang="ts" module>
	/** Sent as the answer when the user dismisses instead of choosing. Mirrors
	 * `SKIP_SENTINEL` in crates/tools/src/origin.rs. */
	export const SKIP_VALUE = '__skip__';

	export type AskOption = string | { label: string; description?: string; recommended?: boolean };

	export interface AskWidgetDef {
		/** 'options' is canonical; legacy single-choice shapes still render. */
		type: 'options' | 'buttons' | 'confirm' | 'select' | 'radio' | 'checkbox' | 'connect_account' | 'install_plugin';
		label?: string;
		options?: AskOption[];
		multiSelect?: boolean;
		default?: string;
		/** connect_account: plugin slug + agent whose account list the OAuth targets.
		 *  install_plugin: same `plugin` slug, plus the marketplace install code. */
		plugin?: string;
		agentId?: string;
		/** install_plugin: PLUG-XXXX-XXXX code redeemed via the canonical POST /codes path. */
		code?: string;
		name?: string;
		description?: string;
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

	import Plug from 'lucide-svelte/icons/plug';
	import Check from 'lucide-svelte/icons/check';
	import Download from 'lucide-svelte/icons/download';
	import { getWebSocketClient } from '$lib/websocket/client';
	import { authLoginAccount, submitCode } from '$lib/api/nebo';

	// connect_account: run the same OAuth pathway as Settings → Connected
	// Accounts, then answer the parked ask_request so the tool call resumes.
	let connecting = $state(false);
	let connectError = $state<string | null>(null);
	let connectDone = $state(false);
	let accountLabel = $state('Primary');

	// install_plugin: redeem the marketplace code through the ONE install
	// pathway (POST /codes → codes::handle_code), then answer the parked
	// ask_request so the discover call resumes.
	let installing = $state(false);
	let installError = $state<string | null>(null);
	let installDone = $state(false);

	async function startInstall(w: AskWidgetDef) {
		if (installing || !w.code) return;
		installing = true;
		installError = null;
		try {
			await submitCode({ code: w.code });
			installDone = true;
			submit('installed');
		} catch (e) {
			installError = e instanceof Error ? e.message : $t('chat.installFailed');
		} finally {
			installing = false;
		}
	}

	async function startConnect(w: AskWidgetDef) {
		if (connecting || !w.plugin || !w.agentId) return;
		connecting = true;
		connectError = null;
		try {
			await authLoginAccount(w.plugin, {
				agentId: w.agentId,
				accountLabel: accountLabel.trim() || 'Primary',
				accountNumber: ''
			});
		} catch {
			connecting = false;
			connectError = $t('chat.connectFailed');
		}
	}

	$effect(() => {
		const w = widgets?.[0];
		if (w?.type !== 'connect_account' || answered || disabled) return;
		const ws = getWebSocketClient();
		const unsubs = [
			ws.on('plugin_auth_complete', (data: Record<string, unknown>) => {
				if ((data.plugin as string) === w.plugin) {
					connecting = false;
					connectDone = true;
					submit('connected');
				}
			}),
			ws.on('plugin_auth_error', (data: Record<string, unknown>) => {
				if ((data.plugin as string) === w.plugin) {
					connecting = false;
					connectError = (data.error as string) || $t('chat.connectFailed');
				}
			}),
		];
		return () => unsubs.forEach((fn) => fn());
	});

	// The prompt is agent/harness-authored text (e.g. the deep-research plan)
	// and uses markdown like every other agent message — render it, don't show
	// raw ** markers. Same marked pipeline as the chat transcript.
	import { parseMarkdown } from '$lib/markdown';
	const promptHtml = $derived(parseMarkdown(prompt));

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
	{:else if widget?.type === 'install_plugin'}
		<div class="flex items-center gap-3 rounded-lg border border-base-300 bg-base-100 px-3 py-2.5">
			<div class="rounded-md bg-base-200 p-2">
				{#if installDone}<Check class="w-5 h-5 text-success" />{:else}<Download class="w-5 h-5" />{/if}
			</div>
			<div class="flex-1 min-w-0">
				<div class="text-sm font-medium truncate">{widget.name ?? widget.plugin}</div>
				{#if installError}
					<div class="text-xs text-error">{installError}</div>
				{:else if widget.description}
					<div class="text-xs text-base-content/60 line-clamp-2">{widget.description}</div>
				{/if}
			</div>
			<button
				type="button"
				class="btn btn-sm btn-primary"
				disabled={installing}
				onclick={() => widget && startInstall(widget)}
			>
				{#if installing}<span class="loading loading-spinner loading-xs"></span>{/if}
				{installing ? $t('chat.installing') : $t('chat.install')}
			</button>
		</div>
		<div class="mt-2 flex">
			<button type="button" class="text-xs text-base-content/40 hover:text-base-content/70 cursor-pointer bg-transparent border-none px-0 ml-auto" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
		</div>
	{:else if widget?.type === 'connect_account'}
		<div class="flex items-center gap-3 rounded-lg border border-base-300 bg-base-100 px-3 py-2.5">
			<div class="rounded-md bg-base-200 p-2">
				{#if connectDone}<Check class="w-5 h-5 text-success" />{:else}<Plug class="w-5 h-5" />{/if}
			</div>
			<div class="flex-1 min-w-0">
				<div class="text-sm font-medium truncate">{widget.label ?? widget.plugin}</div>
				{#if connectError}
					<div class="text-xs text-error">{connectError}</div>
				{:else}
					<div class="text-xs text-base-content/60">{$t('chat.connectAccountHint')}</div>
				{/if}
			</div>
			<button
				type="button"
				class="btn btn-sm btn-primary"
				disabled={connecting}
				onclick={() => widget && startConnect(widget)}
			>
				{#if connecting}<span class="loading loading-spinner loading-xs"></span>{/if}
				{connecting ? $t('chat.connecting') : $t('chat.connect')}
			</button>
		</div>
		<div class="mt-2 flex">
			<button type="button" class="text-xs text-base-content/40 hover:text-base-content/70 cursor-pointer bg-transparent border-none px-0 ml-auto" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
		</div>
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
