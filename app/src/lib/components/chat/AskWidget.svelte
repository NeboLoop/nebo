<script lang="ts" module>
	/** Sent as the answer when the user dismisses instead of choosing. Mirrors
	 * `SKIP_SENTINEL` in crates/tools/src/origin.rs. */
	export const SKIP_VALUE = '__skip__';
	/** Sent as `failed:<the error shown>` when a card's action failed, so the
	 * parked call learns what happened. Mirrors `CARD_FAILED_PREFIX` in
	 * crates/tools/src/plugin_tool.rs. */
	export const FAILED_PREFIX = 'failed:';

	export type AskOption = string | { label: string; description?: string; recommended?: boolean };

	export interface AskWidgetDef {
		/** 'options' is canonical; legacy single-choice shapes still render. */
		type: 'options' | 'buttons' | 'confirm' | 'select' | 'radio' | 'checkbox' | 'connect_account' | 'install_plugin' | 'hire_employee';
		label?: string;
		options?: AskOption[];
		multiSelect?: boolean;
		/** 'select': one pick from a long list, shown as a dropdown. Absent or
		 *  'buttons': a button per option. */
		style?: 'buttons' | 'select';
		default?: string;
		/** connect_account: plugin slug + agent whose account list the OAuth targets.
		 *  install_plugin: same `plugin` slug, plus the marketplace install code. */
		plugin?: string;
		agentId?: string;
		/** connect_account: the employee whose own account this is, for a plugin that
		 *  keeps one per employee; absent for a shared sign-in. */
		employee?: string;
		/** install_plugin / hire_employee: the marketplace code (PLUG-… or AGNT-…) redeemed via
		 *  the canonical POST /codes path — the same button, the same resume, a different verb. */
		code?: string;
		name?: string;
		description?: string;
		/** hire_employee: several listings behind one confirm — the owner asked for a
		 *  team, not one yes-or-no per role. Each is redeemed in sequence through the
		 *  same POST /codes path; `code`/`name` above describe the first for older
		 *  clients. */
		hires?: { code: string; name: string; plugin?: string; description?: string }[];
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
		/** The run ended (stopped or cancelled) with this question unanswered. */
		cancelled?: boolean;
		disabled?: boolean;
		onSubmit: (requestId: string, value: string) => void;
	}

	let { requestId, prompt, widgets, response, cancelled = false, disabled = false, onSubmit }: Props = $props();

	import Plug from 'lucide-svelte/icons/plug';
	import Check from 'lucide-svelte/icons/check';
	import UserPlus from 'lucide-svelte/icons/user-plus';
	import Download from 'lucide-svelte/icons/download';
	import { getWebSocketClient } from '$lib/websocket/client';
	import AskReceipt from './AskReceipt.svelte';
	import { authLoginAccount, listPlugins, submitCode } from '$lib/api/nebo';
	import { isSignInFor } from '$lib/utils/pluginSignIn';
	import CredentialFields, { credentialsComplete, type AuthField } from '$lib/components/CredentialFields.svelte';

	// connect_account: run the same OAuth pathway as Settings → Connected
	// Accounts, then answer the parked ask_request so the tool call resumes.
	let connecting = $state(false);
	let connectError = $state<string | null>(null);
	let connectDone = $state(false);
	let accountLabel = $state('Primary');
	// A plugin that signs in with values (a store domain and a token) instead
	// of a browser: the card asks for them, the same form as the employee's
	// settings. Without them the sign-in can only fail.
	let credFields = $state<AuthField[]>([]);
	let creds = $state<Record<string, string>>({});

	// install_plugin: redeem the marketplace code through the ONE install
	// pathway (POST /codes → codes::handle_code), then answer the parked
	// ask_request so the discover call resumes.
	let installing = $state(false);
	let installError = $state<string | null>(null);
	let installDone = $state(false);
	// The listings on the card (one, or the team) and how many have landed.
	const hireList = $derived((widgets?.[0]?.hires?.length ? widgets[0].hires : widgets?.[0]?.code ? [widgets[0]] : []) as { code?: string; name?: string; description?: string }[]);
	let hiredCodes = $state<Set<string>>(new Set());

	async function startInstall(w: AskWidgetDef) {
		const list = w.hires?.length ? w.hires : w.code ? [w] : [];
		if (installing || list.length === 0) return;
		installing = true;
		installError = null;
		try {
			// One confirm, every code redeemed in sequence through the one install
			// pathway. A failure stops the sequence and names the listing; the
			// button then retries only what is left.
			for (const h of list) {
				if (!h.code || hiredCodes.has(h.code)) continue;
				await submitCode({ code: h.code });
				hiredCodes = new Set([...hiredCodes, h.code]);
			}
			installDone = true;
			submit('installed');
		} catch (e) {
			const failed = list.find((h) => h.code && !hiredCodes.has(h.code));
			const reason = e instanceof Error ? e.message : $t('chat.installFailed');
			installError = failed?.name ? `${failed.name}: ${reason}` : reason;
			fail(installError);
		} finally {
			installing = false;
		}
	}

	async function startConnect(w: AskWidgetDef) {
		if (connecting || !w.plugin || !w.agentId || !credentialsComplete(credFields, creds)) return;
		connecting = true;
		connectError = null;
		try {
			await authLoginAccount(w.plugin, {
				agentId: w.agentId,
				accountLabel: accountLabel.trim() || 'Primary',
				accountNumber: '',
				...(credFields.length ? { credentials: creds } : {})
			});
		} catch (e) {
			// The card stays open with what went wrong: the owner corrects it
			// and signs in again, or skips.
			connecting = false;
			connectError = e instanceof Error && e.message ? e.message : $t('chat.connectFailed');
		}
	}

	$effect(() => {
		const w = widgets?.[0];
		if (w?.type !== 'connect_account' || !w.plugin || answered || disabled) return;
		const slug = w.plugin;
		listPlugins()
			.then((resp) => {
				const p = (resp.plugins as { slug: string; authType?: string; authFields?: AuthField[] }[]).find((x) => x.slug === slug);
				credFields = p?.authType === 'env' ? (p.authFields ?? []) : [];
			})
			.catch(() => {});
	});

	$effect(() => {
		const w = widgets?.[0];
		if (w?.type !== 'connect_account' || answered || disabled) return;
		const ws = getWebSocketClient();
		// A sign-in answers this card only when it is this plugin's and, for an
		// account per employee, this employee's: Chief's sign-in never ticks the
		// Bookkeeper's card.
		const mine = (data: Record<string, unknown>) => isSignInFor(data, w.plugin, w.agentId);
		const unsubs = [
			ws.on('plugin_auth_complete', (data: Record<string, unknown>) => {
				if (mine(data)) {
					connecting = false;
					connectDone = true;
					submit('connected');
				}
			}),
			ws.on('plugin_auth_error', (data: Record<string, unknown>) => {
				if (mine(data)) {
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
	/** The question on one line, for the receipt: its first line, markdown marks dropped. */
	const question = $derived(
		(prompt ?? '')
			.split('\n')
			.map((l) => l.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1').replace(/[*_`#>]/g, '').trim())
			.find(Boolean) ?? ''
	);
	/** What the owner chose, as the receipt marks it: the options picked, and any typed "Other" answer. */
	const chosen = $derived(answered && response !== SKIP_VALUE && !response?.startsWith(FAILED_PREFIX) ? (response ?? '').split(', ') : []);
	const typedAnswers = $derived(options.length > 0 ? chosen.filter((c) => !options.some((o) => o.label === c)) : []);
	const wasSkipped = $derived(response === SKIP_VALUE);
	const failedReason = $derived(response?.startsWith(FAILED_PREFIX) ? response.slice(FAILED_PREFIX.length) : null);

	function submit(value: string) {
		if (!answered && !disabled && !cancelled) {
			onSubmit(requestId, value);
		}
	}

	/** Answer the parked call with the failure the owner sees, on one line. */
	function fail(reason: string) {
		submit(FAILED_PREFIX + reason.replace(/\s+/g, ' ').trim());
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
		if (e.key === 'Escape' && !answered && !disabled && !cancelled) {
			submit(SKIP_VALUE);
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

{#if answered || cancelled || disabled}
	<AskReceipt
		lead={answered && !wasSkipped ? $t('chat.askedReceipt') : cancelled && !answered ? $t('common.cancelled') : $t('common.skipped')}
		subject={question}
		answer={answered && !wasSkipped ? (failedReason ?? response) : ''}
		failed={failedReason != null}
	>
		<div class="prose prose-sm max-w-none [&_p]:my-1 [&>:first-child]:mt-0 [&>:last-child]:mb-0">{@html promptHtml}</div>
		{#if options.length > 0 || typedAnswers.length > 0}
			<ul class="ask-receipt-options">
				{#each options as option}
					<li class="ask-receipt-option" class:ask-receipt-chosen={chosen.includes(option.label)}>
						{#if chosen.includes(option.label)}<Check class="ask-receipt-mark" />{:else}<span class="ask-receipt-mark-none"></span>{/if}
						{option.label}
					</li>
				{/each}
				{#each typedAnswers as typed}
					<li class="ask-receipt-option ask-receipt-chosen"><Check class="ask-receipt-mark" />{typed}</li>
				{/each}
			</ul>
		{/if}
	</AskReceipt>
{:else}
<div class="ask-card">
	<div class="text-sm font-medium mb-2 prose prose-sm max-w-none [&_p]:my-1 [&>:first-child]:mt-0 [&>:last-child]:mb-0">{@html promptHtml}</div>

	{#if widget?.type === 'install_plugin' || widget?.type === 'hire_employee'}
		{@const hiring = widget.type === 'hire_employee'}
		<div class="flex items-center gap-3 rounded-lg border border-base-300 bg-base-100 px-3 py-2.5">
			<div class="rounded-md bg-base-200 p-2">
				{#if installDone}<Check class="w-5 h-5 text-success" />{:else if hiring}<UserPlus class="w-5 h-5" />{:else}<Download class="w-5 h-5" />{/if}
			</div>
			<div class="flex-1 min-w-0">
				{#if hireList.length > 1}
					<ul class="text-sm font-medium space-y-0.5">
						{#each hireList as h (h.code)}
							<li class="flex items-center gap-1.5 truncate">
								{#if h.code && hiredCodes.has(h.code)}<Check class="w-3.5 h-3.5 text-success shrink-0" />{/if}
								<span class="truncate">{h.name}</span>
							</li>
						{/each}
					</ul>
				{:else}
					<div class="text-sm font-medium truncate">{widget.name ?? widget.plugin}</div>
				{/if}
				{#if installError}
					<div class="text-xs text-error">{installError}</div>
				{:else if hireList.length <= 1 && widget.description}
					<div class="text-xs text-base-content/70 line-clamp-2">{widget.description}</div>
				{/if}
			</div>
			<button
				type="button"
				class="btn btn-sm btn-primary"
				disabled={installing}
				onclick={() => widget && startInstall(widget)}
			>
				{#if installing}<span class="loading loading-spinner loading-xs"></span>{/if}
				{#if hiring}
					{#if hireList.length > 1}{installing ? $t('chat.hiringCount', { values: { done: hiredCodes.size, count: hireList.length } }) : $t('chat.hireCount', { values: { count: hireList.length } })}{:else}{installing ? $t('chat.hiring') : $t('chat.hire')}{/if}
				{:else}{installing ? $t('chat.installing') : $t('chat.install')}{/if}
			</button>
		</div>
		<div class="mt-2 flex">
			<button type="button" class="text-xs text-base-content/70 hover:text-base-content cursor-pointer bg-transparent border-none px-0 ms-auto" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
		</div>
	{:else if widget?.type === 'connect_account'}
		<div class="flex items-center gap-3 rounded-lg border border-base-300 bg-base-100 px-3 py-2.5">
			<div class="rounded-md bg-base-200 p-2">
				{#if connectDone}<Check class="w-5 h-5 text-success" />{:else}<Plug class="w-5 h-5" />{/if}
			</div>
			<div class="flex-1 min-w-0">
				<div class="text-sm font-medium truncate">{widget.employee ? $t('chat.connectAccountFor', { values: { label: widget.label ?? widget.plugin, employee: widget.employee } }) : (widget.label ?? widget.plugin)}</div>
				{#if connectError}
					<div class="text-xs text-error">{connectError}</div>
				{:else}
					<div class="text-xs text-base-content/70">{$t('chat.connectAccountHint')}</div>
				{/if}
			</div>
			<button
				type="button"
				class="btn btn-sm btn-primary"
				disabled={connecting || !credentialsComplete(credFields, creds)}
				onclick={() => widget && startConnect(widget)}
			>
				{#if connecting}<span class="loading loading-spinner loading-xs"></span>{/if}
				{connecting ? $t('chat.connecting') : $t('chat.connect')}
			</button>
		</div>
		{#if credFields.length > 0 && !connectDone}
			<div class="mt-2 flex flex-col gap-2">
				<CredentialFields fields={credFields} bind:values={creds} disabled={connecting} onenter={() => widget && startConnect(widget)} />
			</div>
		{/if}
		<div class="mt-2 flex">
			<button type="button" class="text-xs text-base-content/70 hover:text-base-content cursor-pointer bg-transparent border-none px-0 ms-auto" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
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
								{#if option.recommended}<span class="badge badge-primary badge-xs ms-1">{$t('chat.recommended')}</span>{/if}
							</span>
							{#if option.description}<span class="text-xs text-base-content/70">{option.description}</span>{/if}
						</span>
					</label>
				{/each}
			</div>
		{:else if widget?.style === 'select'}
			<select class="select select-bordered select-sm ask-select" onchange={(e) => e.currentTarget.value && submit(e.currentTarget.value)}>
				<option value="" selected disabled>{$t('chat.chooseOne')}</option>
				{#each options as option}
					<option value={option.label}>{option.label}{option.recommended ? ` · ${$t('chat.recommended')}` : ''}</option>
				{/each}
			</select>
		{:else}
			<div class="flex flex-col gap-1.5">
				{#each options as option}
					<button
						type="button"
						class="btn btn-sm btn-outline justify-start h-auto py-1.5 normal-case"
						onclick={() => submit(option.label)}
					>
						<span class="flex flex-col items-start text-start">
							<span class="font-medium">
								{option.label}
								{#if option.recommended}<span class="badge badge-primary badge-xs ms-1">{$t('chat.recommended')}</span>{/if}
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
					<button type="button" class="text-xs text-base-content/80 hover:text-base-content cursor-pointer bg-transparent border-none px-0" onclick={() => (showOther = true)}>{$t('chat.other')}</button>
				{/if}
				<button type="button" class="text-xs text-base-content/70 hover:text-base-content cursor-pointer bg-transparent border-none px-0 ms-auto" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
			</div>
		</div>
	{/if}
</div>
{/if}
