<!--
  UpdateRow — one update: its title, time, sentence, its link, a dot while
  unread. One the assistant handled under a rule also says why (Why? opens
  the rule) and offers Undo where what was done can be undone. The ONE update
  row: the Inbox's groups and a rule's "See what <assistant> did" both draw it.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import Archive from 'lucide-svelte/icons/archive';
  import Undo2 from 'lucide-svelte/icons/undo-2';
  import RuleWhy from './RuleWhy.svelte';
  import { markAsRead } from '$lib/stores/notifications';
  import { addToast } from '$lib/stores/toast';
  import { formatRelative } from '$lib/time';
  import { answerItem, cardKindOf, ruleIdOf, undoOf, type InboxItem } from '$lib/inbox/item';

  let {
    item,
    assistantName,
    why = true,
    onopen,
    onnavigate,
    onputaway
  }: {
    item: InboxItem;
    assistantName: string;
    /** Offer Why? (not on the rule's own page, which is the why). */
    why?: boolean;
    /** The row itself was opened. */
    onopen: (n: InboxItem) => void;
    /** Go to a link. */
    onnavigate: (link: string) => void;
    /** Put it away; no button when absent. */
    onputaway?: (id: string) => void;
  } = $props();

  let current = $state<InboxItem | null>(null);
  const n = $derived(current ?? item);
  const handled = $derived(cardKindOf(item) === 'handled' || item.type === 'handled');
  const ruleId = $derived(ruleIdOf(item));
  const u = $derived(undoOf(n));
  let whyOpen = $state(false);
  let busy = $state(false);

  async function undo() {
    if (!u || busy) return;
    busy = true;
    try {
      current = await answerItem(n.id, u.value);
    } catch (e) {
      addToast((e as Error)?.message || $t('inbox.answerFailed'), 'error');
    } finally {
      busy = false;
    }
  }
</script>

<div class="inbox-update group">
  <span class="inbox-dot {n.readAt ? '' : 'inbox-dot-unread'}" aria-label={n.readAt ? undefined : $t('inbox.unread')}></span>
  <div class="flex-1 min-w-0">
    <button type="button" class="inbox-update-main" onclick={() => onopen(n)}>
      <span class="text-sm truncate {n.readAt ? 'font-medium' : 'font-semibold'}">{n.title}</span>
      <span class="text-xs text-base-content/50 font-mono shrink-0 ms-auto">{formatRelative(n.createdAt, 'short')}</span>
    </button>
    {#if n.body}<p class="text-xs text-base-content/70 mt-0.5">{n.body}</p>{/if}
    {#if (n.actionUrl && n.linkLabel) || (handled && why && ruleId) || u || n.status === 'answered'}
      <div class="flex flex-wrap items-center gap-1.5 mt-1.5">
        {#if n.actionUrl && n.linkLabel}
          <button type="button" class="inbox-link" onclick={() => { markAsRead(n.id); if (n.actionUrl) onnavigate(n.actionUrl); }}>↗ {n.linkLabel}</button>
        {/if}
        {#if handled && why && ruleId}
          <button type="button" class="inbox-link" aria-expanded={whyOpen} onclick={() => (whyOpen = !whyOpen)}>{$t('inbox.why')}</button>
        {/if}
        {#if u && n.status !== 'answered'}
          <button type="button" class="inbox-link" disabled={busy} onclick={undo}><Undo2 class="w-3 h-3" />{u.label}</button>
        {:else if n.status === 'answered' && n.answer}
          <span class="text-xs text-base-content/60">{n.answer}</span>
        {/if}
      </div>
    {/if}
    {#if whyOpen && ruleId}
      <RuleWhy {ruleId} {assistantName} {onnavigate} />
    {/if}
  </div>
  {#if onputaway}
    <button type="button" class="inbox-put-away" onclick={() => onputaway(n.id)} aria-label={$t('inbox.putAwayAction')} title={$t('inbox.putAwayAction')}>
      <Archive class="w-3 h-3" />
    </button>
  {/if}
</div>
