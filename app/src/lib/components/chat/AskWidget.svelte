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
		/** A question card's step: the few words on its step chip, and its
		 *  question (`ask_owner`'s `questions`). A card from before the
		 *  wizard has neither: its question is the ask's prompt. */
		header?: string;
		question?: string;
		/** What the owner is deciding on (markdown): the brief, the draft, the
		 *  plan the question is about, shown on the card above the question
		 *  (`ask_owner`'s `context`). */
		context?: string;
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
		/** A linked agent's own permission question: the owner's answer each
		 *  option is. It takes only those, never his own words. */
		answers?: (string | null)[];
		/** A question Nebo itself asks words itself in the owner's language
		 *  (`AskWords`): questionKey, questionValues, optionKeys. */
		questionKey?: string;
		questionValues?: Record<string, string | number>;
		optionKeys?: (string | null)[];
	}

	/** What a connect card's Connect signs in: the card's own plugin, for the
	 *  employee the card names (`agentId`), wherever the card is shown (the
	 *  employee's chat, or the assistant's chat it is relayed to). Never the
	 *  employee of the chat it happens to sit in. */
	export function connectTarget(w: AskWidgetDef): { plugin: string; agentId: string } | null {
		return w.type === 'connect_account' && w.plugin && w.agentId ? { plugin: w.plugin, agentId: w.agentId } : null;
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
		/** Shown in place of the composer: Escape closes the card, unanswered,
		 *  instead of skipping the question. */
		ondismiss?: () => void;
		/** The employee who asks, drawn on the card. */
		asker?: { id: string; name: string } | null;
		/** Stop the employee's work, from the card's corner. */
		onstop?: () => void;
	}

	let { requestId, prompt, widgets, response, cancelled = false, disabled = false, onSubmit, ondismiss, asker = null, onstop }: Props = $props();

	import Plug from 'lucide-svelte/icons/plug';
	import Check from 'lucide-svelte/icons/check';
	import UserPlus from 'lucide-svelte/icons/user-plus';
	import Download from 'lucide-svelte/icons/download';
	import { getWebSocketClient } from '$lib/websocket/client';
	import AskReceipt from './AskReceipt.svelte';
	import { authLoginAccount, listPlugins, submitCode } from '$lib/api/nebo';
	import { isSignInFor, openSignInLink } from '$lib/utils/pluginSignIn';
	import CredentialFields, { credentialsComplete, type AuthField } from '$lib/components/CredentialFields.svelte';
	import DecisionCard from '$lib/components/DecisionCard.svelte';
	import { questionOrder, type DecisionChoice } from '$lib/chat/decision';

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
		const target = connectTarget(w);
		if (connecting || !target || !credentialsComplete(credFields, creds)) return;
		connecting = true;
		connectError = null;
		try {
			const resp = await authLoginAccount(target.plugin, {
				agentId: target.agentId,
				accountLabel: accountLabel.trim() || 'Primary',
				accountNumber: '',
				...(credFields.length ? { credentials: creds } : {})
			});
			openSignInLink(resp);
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
	import { parseMarkdown, plainText } from '$lib/markdown';
	import { wordedOption, wordedQuestion } from '$lib/chat/askWords';
	import { chooseOther, emptyAnswer, pick, replyAnswers, replyValue, stepValue, wizardSteps, type StepAnswer, type WizardStep } from '$lib/chat/askWizard';
	const widget = $derived(widgets?.[0]);
	// The question in the owner's language when Nebo sent its key, else as
	// written; its context as written.
	const shownPrompt = $derived(wordedQuestion($t, prompt, widget));
	const shownContext = $derived(widget?.context ?? '');
	const promptHtml = $derived(parseMarkdown(shownPrompt));
	const contextHtml = $derived(shownContext ? parseMarkdown(shownContext) : '');

	// A question card's steps: one per question, each answered in turn and
	// all sent together ($lib/chat/askWizard).
	const steps = $derived(wizardSteps(prompt, widgets));
	let at = $state(0);
	let stepAnswers = $state<StepAnswer[]>([]);
	const step = $derived(steps[Math.min(at, steps.length - 1)]);
	const current = $derived(stepAnswers[at] ?? emptyAnswer());
	const options = $derived(step?.options ?? []);
	const optionTextOf = (s: WizardStep | undefined, label: string) => wordedOption($t, (s?.options ?? []).map((o) => o.label), label, s?.widget);
	/** An option's label as the owner reads it; the answer is the label. */
	const optionText = (label: string) => optionTextOf(step, label);
	const stepQuestionHtml = $derived(step ? parseMarkdown(wordedQuestion($t, step.question, step.widget)) : '');
	const isLast = $derived(at >= steps.length - 1);
	const stepAnswered = $derived(step ? stepValue(step, current) !== '' : false);
	/** A card done in the app (an install, a hire, a sign-in). */
	const inApp = $derived(widget?.type === 'install_plugin' || widget?.type === 'hire_employee' || widget?.type === 'connect_account');

	/** "Other": his own words answer the step. */
	const OTHER = '\u0000other';
	/** The step's choices as the card's buttons: the recommended one first
	 *  and filled, the rest outlined, then Other. A long list is a dropdown
	 *  instead. */
	const questionChoices = $derived<DecisionChoice[]>(
		step && !step.select
			? [
					...questionOrder(options).map((o, i): DecisionChoice => ({
						id: o.label,
						label: optionText(o.label),
						description: o.description,
						tone: i === 0 && !step.multi ? 'filled' : 'outlined',
						on: current.picked.includes(o.label),
						badge: o.recommended ? $t('chat.recommended') : undefined
					})),
					...(!step.widget.answers && options.length > 0
						? [{ id: OTHER, label: $t('chat.askOther'), tone: 'outlined' as const, on: current.otherOn }]
						: [])
				]
			: []
	);
	/** The owner picks, then sends with Next or Submit: several picks, a
	 *  dropdown, his own words, or a card of several questions. A single
	 *  pick on a card of one question answers it at once. */
	const picksFirst = $derived(!!step && (steps.length > 1 || step.multi || step.select || current.otherOn || options.length === 0));

	/** A tap on one of the step's buttons. */
	function choose(id: string) {
		if (!step) return;
		if (id === OTHER) {
			setAnswer(chooseOther(step, current));
			return;
		}
		setAnswer(pick(step, current, id));
		if (step.multi) return;
		if (isLast) sendAll();
		else go(at + 1);
	}

	function setAnswer(a: StepAnswer) {
		const next = [...stepAnswers];
		next[at] = a;
		stepAnswers = next;
	}

	function go(to: number) {
		at = Math.max(0, Math.min(steps.length - 1, to));
	}

	function sendAll() {
		if (steps.every((s, i) => stepValue(s, stepAnswers[i] ?? emptyAnswer()) !== '')) submit(replyValue(steps, stepAnswers));
	}

	const answered = $derived(response != null);
	/** The question on one line, for the receipt: its first line, markdown marks dropped. */
	const question = $derived(steps.length > 1 ? steps.map((s) => s.header).join(', ') : plainText(shownPrompt ?? '', { first: true }));
	const wasSkipped = $derived(response === SKIP_VALUE);
	const failedReason = $derived(response?.startsWith(FAILED_PREFIX) ? response.slice(FAILED_PREFIX.length) : null);
	/** Each step's answer as it was sent. */
	const sent = $derived(answered && !wasSkipped && failedReason == null ? replyAnswers(steps, response ?? '') : []);
	/** What the owner chose on step `i`, as the receipt marks it: the options
	 *  picked, and any answer typed in his own words. */
	const chosenOf = (i: number) => (sent[i] ?? '') === '' ? [] : sent[i].split(', ');
	const typedOf = (i: number) => steps[i]?.options.length ? chosenOf(i).filter((c) => !steps[i].options.some((o) => o.label === c)) : chosenOf(i);
	/** The receipt's answer: each step's, as he reads it. */
	const receiptAnswer = $derived(
		failedReason ?? sent.map((v, i) => (v ? chosenOf(i).map((c) => optionTextOf(steps[i], c)).join(', ') : '')).filter(Boolean).join(' · ')
	);

	function submit(value: string) {
		if (!answered && !disabled && !cancelled) {
			onSubmit(requestId, value);
		}
	}

	/** Answer the parked call with the failure the owner sees, on one line. */
	function fail(reason: string) {
		submit(FAILED_PREFIX + reason.replace(/\s+/g, ' ').trim());
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && !answered && !disabled && !cancelled) {
			if (ondismiss) ondismiss();
			else submit(SKIP_VALUE);
		}
	}
</script>

<svelte:window onkeydown={onKeydown} />

{#if answered || cancelled || disabled}
	<AskReceipt
		lead={answered && !wasSkipped ? $t('chat.askedReceipt') : cancelled && !answered ? $t('common.cancelled') : $t('common.skipped')}
		subject={question}
		answer={answered && !wasSkipped ? receiptAnswer : ''}
		failed={failedReason != null}
	>
		{#if contextHtml}<div class="ask-context prose prose-sm max-w-none">{@html contextHtml}</div>{/if}
		{#each steps as s, i}
			<div class="prose prose-sm max-w-none [&_p]:my-1 [&>:first-child]:mt-0 [&>:last-child]:mb-0">{@html parseMarkdown(wordedQuestion($t, s.question, s.widget))}</div>
			{#if s.options.length > 0 || typedOf(i).length > 0}
				<ul class="ask-receipt-options">
					{#each s.options as option}
						<li class="ask-receipt-option" class:ask-receipt-chosen={chosenOf(i).includes(option.label)}>
							{#if chosenOf(i).includes(option.label)}<Check class="ask-receipt-mark" />{:else}<span class="ask-receipt-mark-none"></span>{/if}
							{optionTextOf(s, option.label)}
						</li>
					{/each}
					{#each typedOf(i) as typed}
						<li class="ask-receipt-option ask-receipt-chosen"><Check class="ask-receipt-mark" />{typed}</li>
					{/each}
				</ul>
			{/if}
		{/each}
	</AskReceipt>
{:else if widget?.type === 'install_plugin' || widget?.type === 'hire_employee'}
	{@const hiring = widget.type === 'hire_employee'}
	<!-- Done in the app: stays in the chat, on the same card. -->
	<DecisionCard
		line={hireList.length > 1 ? [] : [{ text: widget.name ?? widget.plugin ?? '', bold: true }]}
		title={hireList.length > 1 ? hires : undefined}
		reason={installError ?? (hireList.length <= 1 ? widget.description ?? '' : '')}
		choices={[
			{
				id: 'go',
				tone: 'filled',
				working: installing,
				label: hiring
					? hireList.length > 1
						? installing ? $t('chat.hiringCount', { values: { done: hiredCodes.size, count: hireList.length } }) : $t('chat.hireCount', { values: { count: hireList.length } })
						: installing ? $t('chat.hiring') : $t('chat.hire')
					: installing ? $t('chat.installing') : $t('chat.install')
			},
			{ id: SKIP_VALUE, tone: 'outlined', label: $t('common.skip') }
		]}
		onchoose={(id) => (id === 'go' ? widget && startInstall(widget) : submit(SKIP_VALUE))}
		{body}
	>
		{#snippet icon()}{#if installDone}<Check class="w-5 h-5 text-success" />{:else if hiring}<UserPlus class="w-5 h-5" />{:else}<Download class="w-5 h-5" />{/if}{/snippet}
	</DecisionCard>
{:else if widget?.type === 'connect_account'}
	<DecisionCard
		line={[{ text: widget.employee ? $t('chat.connectAccountFor', { values: { label: widget.label ?? widget.plugin ?? '', employee: widget.employee } }) : (widget.label ?? widget.plugin ?? ''), bold: true }]}
		reason={connectError ?? $t('chat.connectAccountHint')}
		choices={[
			{ id: 'go', tone: 'filled', working: connecting, disabled: !credentialsComplete(credFields, creds), label: connecting ? $t('chat.connecting') : $t('chat.connect') },
			{ id: SKIP_VALUE, tone: 'outlined', label: $t('common.skip') }
		]}
		onchoose={(id) => (id === 'go' ? widget && startConnect(widget) : submit(SKIP_VALUE))}
		{body}
	>
		{#snippet icon()}{#if connectDone}<Check class="w-5 h-5 text-success" />{:else}<Plug class="w-5 h-5" />{/if}{/snippet}
		{#if credFields.length > 0 && !connectDone}
			<div class="decision-fields">
				<CredentialFields fields={credFields} bind:values={creds} disabled={connecting} onenter={() => widget && startConnect(widget)} />
			</div>
		{/if}
	</DecisionCard>
{:else if step}
	<DecisionCard
		who={asker}
		{title}
		{body}
		choices={questionChoices}
		multi={step.multi}
		onchoose={choose}
		{onstop}
	>
		{#if step.select}
			<select
				class="select select-bordered select-sm ask-select"
				value={current.otherOn ? '' : (current.picked[0] ?? '')}
				onchange={(e) => e.currentTarget.value && setAnswer(pick(step, current, e.currentTarget.value))}
			>
				<option value="" disabled>{$t('chat.chooseOne')}</option>
				{#each options as option}
					<option value={option.label}>{optionText(option.label)}{option.recommended ? ` · ${$t('chat.recommended')}` : ''}</option>
				{/each}
			</select>
		{/if}
		{#if current.otherOn || options.length === 0}
			<!-- svelte-ignore a11y_autofocus -->
			<input
				type="text"
				class="input input-bordered input-sm ask-other-input"
				placeholder={$t('chat.askOtherPlaceholder')}
				value={current.other}
				autofocus
				oninput={(e) => setAnswer({ ...current, otherOn: true, other: e.currentTarget.value })}
				onkeydown={(e) => {
					if (e.key === 'Enter' && stepAnswered) {
						e.preventDefault();
						if (isLast) sendAll();
						else go(at + 1);
					}
				}}
			/>
		{/if}
		<div class="ask-actions">
			<button type="button" class="ask-skip" onclick={() => submit(SKIP_VALUE)}>{$t('common.skip')}</button>
			{#if steps.length > 1 && at > 0}
				<button type="button" class="btn btn-sm btn-ghost" onclick={() => go(at - 1)}>{$t('common.back')}</button>
			{/if}
			{#if picksFirst}
				{#if isLast}
					<button type="button" class="btn btn-sm btn-primary" disabled={!steps.every((s, i) => stepValue(s, stepAnswers[i] ?? emptyAnswer()) !== '')} onclick={sendAll}>{$t('common.submit')}</button>
				{:else}
					<button type="button" class="btn btn-sm btn-primary" disabled={!stepAnswered} onclick={() => go(at + 1)}>{$t('common.next')}</button>
				{/if}
			{/if}
		</div>
	</DecisionCard>
{/if}

<!-- The question, as its card's header: what he decides on (the brief,
     the draft) above it, the steps by their headers when there are
     several, then the one on screen. -->
{#snippet title()}
	{#if contextHtml}<div class="ask-context prose prose-sm max-w-none">{@html contextHtml}</div>{/if}
	{#if steps.length > 1}
		<div class="ask-steps" role="tablist">
			{#each steps as s, i}
				<button
					type="button"
					role="tab"
					class="ask-step"
					class:ask-step-current={i === at}
					class:ask-step-done={i !== at && stepValue(s, stepAnswers[i] ?? emptyAnswer()) !== ''}
					aria-selected={i === at}
					onclick={() => go(i)}
				>{s.header}</button>
			{/each}
		</div>
		<div class="ask-question prose prose-sm max-w-none">{@html stepQuestionHtml}</div>
	{:else}
		<div class="ask-question prose prose-sm max-w-none">{@html promptHtml}</div>
	{/if}
	{#if widget?.label}
		<p class="decision-reason">{widget.label}</p>
	{/if}
{/snippet}

<!-- A card done in the app: what it is about, and the employee's own
     words about it. -->
{#snippet body()}
	{#if inApp && contextHtml}<div class="ask-context prose prose-sm max-w-none">{@html contextHtml}</div>{/if}
	{#if inApp && promptHtml}
		<div class="decision-prompt prose prose-sm max-w-none">{@html promptHtml}</div>
	{/if}
{/snippet}

<!-- The team on one hire card, each ticked as it lands. -->
{#snippet hires()}
	<ul class="decision-list">
		{#each hireList as h (h.code)}
			<li class="decision-list-item">
				{#if h.code && hiredCodes.has(h.code)}<Check class="w-3.5 h-3.5 text-success shrink-0" />{/if}
				<span class="truncate">{h.name}</span>
			</li>
		{/each}
	</ul>
{/snippet}
