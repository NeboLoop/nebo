<!--
  NeedsYouCard — one decision waiting on the owner, answerable right on the
  card. The SAME card in the Inbox and in the assistant's chat (its
  `inbox_item` payload): only `via` differs. Its answers are the item's own
  options (buttons, or chips for an employee's own question, which also takes
  the owner's own words); one with none is answered on its own screen, its
  link. The assistant's offer to handle a kind of item (`rule_offer`) spells
  out what the rule covers, what it will do and what still comes to the
  owner. Every answer goes through the one answer path (answerItem), and an
  answer from anywhere settles the card in place.
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
  import { notifications } from '$lib/stores/notifications';
  import { addToast } from '$lib/stores/toast';
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
    assistantName = '',
    via = answerVia(),
    onopen
  }: {
    item: InboxItem;
    /** The employee who asks, in the owner's words. */
    name: string;
    color?: string | null;
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

<div class="inbox-card {offer && open ? 'inbox-card-offer' : ''}" data-testid="needs-you-card">
  <div class="inbox-card-meta">
    <AgentAvatar {name} {color} size="xs" />
    <span class="truncate">{offer ? $t('inbox.yourAssistant', { values: { name: assistantName || name } }) : name}</span>
    <span class="shrink-0">· {formatRelative(current.createdAt, 'short')}</span>
  </div>
  <div class="inbox-card-title">{current.title}</div>
  {#if current.body}<p class="inbox-card-body">{current.body}</p>{/if}

  {#if offer && proposal}
    <dl class="inbox-facts">
      <dt>{$t('inbox.ruleCovers')}</dt><dd>{proposal.covers}</dd>
      {#if proposal.does}<dt>{$t('inbox.ruleDoes')}</dt><dd>{proposal.does}</dd>{/if}
      {#if proposal.stillAsks}<dt>{$t('inbox.ruleStillAsks')}</dt><dd>{proposal.stillAsks}</dd>{/if}
      {#if proposal.bot}<dt>{$t('inbox.ruleBot')}</dt><dd>{proposal.bot}</dd>{/if}
    </dl>
    {#if proposal.cantUndo}<p class="inbox-cant-undo">{$t('inbox.cantUndo')}</p>{/if}
  {/if}

  {#if current.status === 'update'}
    {#if current.actionUrl}
      <div><button type="button" class="inbox-link" onclick={() => current.actionUrl && onopen(current.actionUrl)}>↗ {current.linkLabel || $t('common.open')}</button></div>
    {/if}
  {:else if !open}
    <p class="inbox-answered">
      {current.status === 'archived' ? $t('inbox.putAway') : $t('inbox.youAnswered', { values: { answer: current.answer ?? '' } })}
    </p>
  {:else if options.length === 0}
    <div class="flex items-center gap-2">
      <button type="button" class="btn btn-primary btn-sm" onclick={() => current.actionUrl && onopen(current.actionUrl)}>{$t('inbox.review')}</button>
    </div>
  {:else if offer}
    <div class="grid grid-cols-2 gap-2">
      {#each options as o, i (o.value)}
        <button
          type="button"
          class="btn btn-sm {i === 0 ? 'btn-primary col-span-2' : 'btn-ghost bg-primary/10 text-primary'}"
          disabled={busy}
          onclick={() => answer(o.value)}
        >{o.label}</button>
      {/each}
    </div>
  {:else if ownWords}
    <div class="flex flex-wrap gap-2">
      {#each options as o (o.value)}
        <button type="button" class="inbox-chip" disabled={busy} onclick={() => answer(o.value)}>{o.label}</button>
      {/each}
      {#if !writing}
        <button type="button" class="inbox-chip inbox-chip-quiet" disabled={busy} onclick={() => (writing = true)}>{$t('inbox.somethingElse')}</button>
      {/if}
    </div>
    {#if writing}
      <form class="flex items-center gap-2" onsubmit={(e) => { e.preventDefault(); void answer(words); }}>
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input input-sm flex-1 min-w-0" bind:value={words} placeholder={$t('inbox.yourAnswer')} autofocus />
        <button type="submit" class="btn btn-primary btn-sm" disabled={busy || !words.trim()}>{$t('inbox.send')}</button>
      </form>
    {/if}
  {:else}
    <div class="flex flex-wrap items-center gap-2">
      {#each options as o, i (o.value)}
        <button
          type="button"
          class="btn btn-sm {i === 0 ? 'btn-primary' : 'btn-ghost border border-base-content/15'}"
          disabled={busy}
          onclick={() => answer(o.value)}
        >{o.label}</button>
      {/each}
      {#if current.actionUrl && current.linkLabel}
        <button type="button" class="inbox-link" onclick={() => current.actionUrl && onopen(current.actionUrl)}>↗ {current.linkLabel}</button>
      {/if}
    </div>
  {/if}
</div>
