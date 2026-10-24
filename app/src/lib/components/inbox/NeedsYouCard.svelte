<!--
  NeedsYouCard — one decision waiting on the owner, answerable right on the
  card. The SAME card in the Inbox and in the assistant's chat (its
  `inbox_item` payload): only `via` differs. Its answers are the item's own
  options (buttons, or chips for an employee's own question, which also takes
  the owner's own words); one with none is answered on its own screen, its
  link. The assistant's offer to handle a kind of item (`rule_offer`) spells
  out what the rule covers, what it will do and what still comes to the
  owner. Every answer goes through the one answer path (answerItem), and an
  answer from anywhere settles the card in place. An open decision is the ONE
  decision card (DecisionCard); a permission ask or an approval still waiting
  is its own card, the same one its chat shows.
-->
<script lang="ts" module>
  /** What an offer proposes, as the staged proposal's content carries it. */
  export interface RuleProposal {
    employee?: string;
    covers?: string;
    does?: string;
    stillAsks?: string;
    cantUndo?: boolean;
    bot?: string;
  }

  /** The proposal inside a staged `rule` write, or null when it isn't one. */
  export function proposalOf(learning: unknown): RuleProposal | null {
    const content = (learning as { content?: string | null } | null)?.content;
    if (!content) return null;
    try {
      const p = JSON.parse(content);
      return p && typeof p === 'object' && typeof p.covers === 'string' ? (p as RuleProposal) : null;
    } catch {
      return null;
    }
  }
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import { formatRelative } from '$lib/time';
  import { plainText } from '$lib/markdown';
  import { notifications } from '$lib/stores/notifications';
  import { addToast } from '$lib/stores/toast';
  import { openAsks } from '$lib/stores/permissionAsks';
  import { approvals } from '$lib/stores/approvals';
  import DecisionCard from '$lib/components/DecisionCard.svelte';
  import PermissionAskCard from '$lib/components/PermissionAskCard.svelte';
  import ApprovalAskCard from '$lib/components/chat/ApprovalAskCard.svelte';
  import type { DecisionChoice, DecisionRow } from '$lib/chat/decision';
  import {
    answerItem,
    answerVia,
    answersOf,
    learningIdOf,
    sectionOf,
    takesOwnWords,
    type AnswerVia,
    type InboxItem
  } from '$lib/inbox/item';

  let {
    item,
    name,
    color = null,
    shape = null,
    assistantName = '',
    via = answerVia(),
    onopen
  }: {
    item: InboxItem;
    /** The employee who asks, in the owner's words. */
    name: string;
    color?: string | null;
    /** The character the employee wears. */
    shape?: string | null;
    /** The assistant's configured name, for its own offers. */
    assistantName?: string;
    via?: AnswerVia;
    /** Open the item's link (its own screen). */
    onopen: (link: string) => void;
  } = $props();

  let answered = $state<InboxItem | null>(null);
  // The Inbox's copy wins: an answer from anywhere else settles this card.
  const current = $derived($notifications.find((n) => n.id === item.id) ?? answered ?? item);
  const open = $derived(sectionOf(current) === 'needs_you');
  const offer = $derived(current.type === 'rule_offer');
  const options = $derived(answersOf(current));
  const ownWords = $derived(takesOwnWords(current));

  let busy = $state(false);
  let writing = $state(false);
  let words = $state('');

  let proposal = $state<RuleProposal | null>(null);
  $effect(() => {
    const id = offer ? learningIdOf(current) : null;
    if (!id || proposal) return;
    import('$lib/api/nebo')
      .then((api) => api.getLearning(id))
      .then((l) => (proposal = proposalOf(l)))
      .catch(() => {});
  });

  /** A permission ask or an approval still waiting: its own card. */
  const permission = $derived(open && current.id.startsWith('permission-ask:') ? $openAsks.find((a) => `permission-ask:${a.id}` === current.id) : undefined);
  const approval = $derived(open && current.id.startsWith('approval:') ? $approvals.find((a) => `approval:${a.requestId}` === current.id && !a.decision) : undefined);

  /** "Something else": his own words. */
  const OWN = '\u0000own';
  const REVIEW = '\u0000review';
  /** The item's answers as the card's buttons: the first filled, the rest
   *  outlined; one answered on its own screen is a Review button. */
  const choices = $derived<DecisionChoice[]>(
    options.length === 0
      ? [{ id: REVIEW, label: $t('inbox.review'), tone: 'filled' }]
      : [
          ...options.map((o, i): DecisionChoice => ({ id: o.value, label: o.label, tone: i === 0 ? 'filled' : 'outlined' })),
          ...(ownWords && !writing ? [{ id: OWN, label: $t('inbox.somethingElse'), tone: 'outlined' as const }] : [])
        ]
  );
  /** An offer's terms, folded behind "Show details". */
  const offerRows = $derived<DecisionRow[]>(
    offer && proposal
      ? [
          { label: $t('inbox.ruleCovers'), value: proposal.covers ?? '' },
          ...(proposal.does ? [{ label: $t('inbox.ruleDoes'), value: proposal.does }] : []),
          ...(proposal.stillAsks ? [{ label: $t('inbox.ruleStillAsks'), value: proposal.stillAsks }] : []),
          ...(proposal.bot ? [{ label: $t('inbox.ruleBot'), value: proposal.bot }] : [])
        ]
      : []
  );

  function choose(id: string) {
    if (id === OWN) writing = true;
    else if (id === REVIEW) {
      if (current.actionUrl) onopen(current.actionUrl);
    } else void answer(id);
  }

  async function answer(value: string) {
    if (busy || !value.trim()) return;
    busy = true;
    try {
      answered = await answerItem(current.id, value.trim(), via);
      writing = false;
    } catch (e) {
      addToast((e as Error)?.message || $t('inbox.answerFailed'), 'error');
    } finally {
      busy = false;
    }
  }
</script>

{#if permission}
  <PermissionAskCard ask={permission} via="inbox" />
{:else if approval}
  <ApprovalAskCard {approval} />
{:else if open}
  <DecisionCard
    who={{ id: current.agentId ?? '', name, color, shape }}
    line={[
      { text: offer ? $t('inbox.yourAssistant', { values: { name: assistantName || name } }) : name, bold: true },
      { text: ` · ${formatRelative(current.createdAt, 'short')}`, bold: false }
    ]}
    action={current.title}
    reason={current.body ? plainText(current.body) : ''}
    rows={offerRows}
    {choices}
    {busy}
    onchoose={choose}
  >
    {#if offer && proposal?.cantUndo}<p class="inbox-cant-undo">{$t('inbox.cantUndo')}</p>{/if}
    {#if writing}
      <form class="flex items-center gap-2" onsubmit={(e) => { e.preventDefault(); void answer(words); }}>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input input-sm flex-1 min-w-0" bind:value={words} placeholder={$t('inbox.yourAnswer')} autofocus />
        <button type="submit" class="btn btn-primary btn-sm" disabled={busy || !words.trim()}>{$t('inbox.send')}</button>
      </form>
    {/if}
    {#if options.length > 0 && current.actionUrl && current.linkLabel}
      <button type="button" class="decision-link" onclick={() => current.actionUrl && onopen(current.actionUrl)}>↗ {current.linkLabel}</button>
    {/if}
  </DecisionCard>
{:else}
<div class="inbox-card" data-testid="needs-you-card">
  <div class="inbox-card-meta">
    <AgentAvatar id={current.agentId ?? ''} {name} {color} {shape} size="sm" />
    <span class="truncate">{offer ? $t('inbox.yourAssistant', { values: { name: assistantName || name } }) : name}</span>
    <span class="shrink-0">· {formatRelative(current.createdAt, 'short')}</span>
  </div>
  <div class="inbox-card-title">{current.title}</div>
  {#if current.body}<p class="inbox-card-body">{plainText(current.body)}</p>{/if}

  {#if current.status === 'update'}
    {#if current.actionUrl}
      <div><button type="button" class="inbox-link" onclick={() => current.actionUrl && onopen(current.actionUrl)}>↗ {current.linkLabel || $t('common.open')}</button></div>
    {/if}
  {:else}
    <p class="inbox-answered">
      {current.status === 'archived' ? $t('inbox.putAway') : $t('inbox.youAnswered', { values: { answer: current.answer ?? '' } })}
    </p>
  {/if}
</div>
{/if}
