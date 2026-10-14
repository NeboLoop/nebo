<!--
  The Inbox: where the employees leave updates for the owner, and the
  decisions that need him. Two kinds of item, both the server's rows:
  a decision (Needs you), answered right on its card, which moves to History
  once answered; and an update (a title, one sentence, a link to the work),
  grouped by employee, with a dot while unread. The assistant's morning brief
  sits at the top, with the way into its chat. History holds what was
  answered and what was put away; nothing is deleted.

  An item about no other place opens here, in the reader (its full text, a
  proposal's staged change, a failed run's trail). Selection is owned by
  whoever mounts this (the shelf keeps it in `?m=`).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { parseMarkdown } from '$lib/markdown';
  import SearchIcon from 'lucide-svelte/icons/search';
  import X from 'lucide-svelte/icons/x';
  import Copy from 'lucide-svelte/icons/copy';
  import Check from 'lucide-svelte/icons/check';
  import Archive from 'lucide-svelte/icons/archive';
  import ArrowLeft from 'lucide-svelte/icons/arrow-left';
  import ExternalLink from 'lucide-svelte/icons/external-link';
  import MoreHorizontal from 'lucide-svelte/icons/more-horizontal';
  import ChevronDown from 'lucide-svelte/icons/chevron-down';
  import Undo2 from 'lucide-svelte/icons/undo-2';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import NeedsYouCard from './NeedsYouCard.svelte';
  import RuleWhy from './RuleWhy.svelte';
  import ProposedChange, { type Learning } from './ProposedChange.svelte';
  import {
    notifications, history, inboxCounts, hasMore, loadNotifications, loadMore, loadHistory,
    markAsRead, markAllRead, removeNotification,
  } from '$lib/stores/notifications';
  import {
    ASSISTANT_ID, answerItem, canRevertLearning, cardKindOf, groupUpdates, isAssistant, learningIdOf,
    ruleIdOf, sectionOf, splitGroups, surfaceKey, topNeedsYou, undoOf, type InboxItem,
  } from '$lib/inbox/item';
  import { rulesPath } from '$lib/inbox/rules';
  import { currentUser } from '$lib/stores/auth';
  import { addToast } from '$lib/stores/toast';
  import { formatRelative, greetingKey } from '$lib/time';
  import { getLearning, getRun, revertLearning } from '$lib/api/nebo';
  import type { GetRunResponse, PendingTask } from '$lib/api/neboComponents';

  let {
    selectedId = null,
    embedded = false,
    onselect,
    onnavigate
  }: {
    selectedId?: string | null;
    /** True when a modal already supplies the title — don't print it twice. */
    embedded?: boolean;
    onselect: (id: string | null) => void;
    onnavigate: (link: string) => void;
  } = $props();

  // id → {name, colour} for every card's who-line and the employee filters.
  let roster = $state<Record<string, { name: string; color?: string | null }>>({});
  onMount(async () => {
    loadNotifications();
    try {
      const { listAgents } = await import('$lib/api/nebo');
      const res = await listAgents(200);
      const map: Record<string, { name: string; color?: string | null }> = {};
      for (const a of (res.agents || []) as Array<{ id: string; name: string; color?: string | null }>) {
        map[a.id] = { name: a.name, color: a.color };
      }
      roster = map;
    } catch { /* names are progressive enhancement */ }
  });

  const assistantName = $derived(roster[ASSISTANT_ID]?.name ?? '');
  const who = (agentId?: string | null) =>
    isAssistant(agentId) ? roster[ASSISTANT_ID] ?? { name: assistantName } : agentId ? roster[agentId] ?? { name: $t('inbox.formerEmployee') } : { name: $t('inbox.fromNebo') };

  // ── Filters: one part of the Inbox, or one employee's items. ──
  type Filter = 'all' | 'needs_you' | 'updates' | 'history' | `agent:${string}`;
  let filter = $state<Filter>('all');
  let search = $state('');
  let moreNeeds = $state(false);
  let moreGroups = $state(false);
  let whyOpen = $state<string | null>(null);

  function pick(f: Filter) {
    filter = filter === f ? 'all' : f;
    if (filter === 'history') void loadHistory();
  }

  const matches = (n: InboxItem) => {
    const q = search.trim().toLowerCase();
    return !q || n.title.toLowerCase().includes(q) || (n.body ?? '').toLowerCase().includes(q);
  };
  const agentFilter = $derived(filter.startsWith('agent:') ? filter.slice('agent:'.length) : null);
  const inView = $derived(
    $notifications.filter(matches).filter((n) => !agentFilter || (isAssistant(agentFilter) ? isAssistant(n.agentId) : n.agentId === agentFilter))
  );

  const brief = $derived($notifications.find((n) => sectionOf(n) === 'brief') ?? null);
  const needs = $derived(topNeedsYou(inView, moreNeeds || filter !== 'all'));
  const groups = $derived(groupUpdates(inView));
  const split = $derived(splitGroups(groups, moreGroups || filter !== 'all'));
  const pastItems = $derived($history.filter(matches));

  /** Employees with something in the Inbox, for their own filter. */
  const employees = $derived(
    [...new Set($notifications.filter((n) => sectionOf(n) !== 'history' && n.agentId).map((n) => (isAssistant(n.agentId) ? ASSISTANT_ID : n.agentId!)))]
      .map((id) => ({ id, name: who(id).name }))
      .filter((e) => e.name)
      .sort((a, b) => a.name.localeCompare(b.name))
  );

  const firstName = $derived($currentUser?.name?.split(' ')[0] ?? '');
  const greeting = $derived(
    firstName ? $t('chat.greetingWithName', { values: { greeting: $t(greetingKey()), name: firstName } }) : $t(greetingKey())
  );

  // ── Infinite scroll: the sentinel under the updates pulls the next page. ──
  let sentinel = $state<HTMLElement | null>(null);
  let loadingMore = false;
  $effect(() => {
    if (!sentinel) return;
    const io = new IntersectionObserver(async (entries) => {
      if (entries.some((e) => e.isIntersecting) && !loadingMore) {
        loadingMore = true;
        await loadMore();
        loadingMore = false;
      }
    });
    io.observe(sentinel);
    return () => io.disconnect();
  });

  /** Whether a link names a place other than the Inbox itself. */
  const elsewhere = (link?: string): link is string =>
    !!link && !link.startsWith('/inbox') && !link.startsWith('/dashboard?inbox');

  /** An update opens the place it is about; one about no other place opens
   *  here, in the reader. */
  function open(n: InboxItem) {
    markAsRead(n.id);
    if (elsewhere(n.actionUrl)) {
      onnavigate(n.actionUrl);
      return;
    }
    copied = false;
    onselect(n.id);
  }

  function putAway(id: string) {
    removeNotification(id);
    if (selectedId === id) onselect(null);
  }

  let busy = $state<Record<string, boolean>>({});
  async function undo(n: InboxItem) {
    const u = undoOf(n);
    if (!u || busy[n.id]) return;
    busy = { ...busy, [n.id]: true };
    try {
      await answerItem(n.id, u.value);
    } catch (e) {
      addToast((e as Error)?.message || $t('inbox.answerFailed'), 'error');
    } finally {
      busy = { ...busy, [n.id]: false };
    }
  }

  /** An approved proposal undone from History, through the path that applied it. */
  async function revert(n: InboxItem) {
    const id = learningIdOf(n);
    if (!id || busy[n.id]) return;
    busy = { ...busy, [n.id]: true };
    try {
      const r = await revertLearning(id);
      reverted = { ...reverted, [n.id]: (r as { status?: string }).status ?? 'reverted' };
    } finally {
      busy = { ...busy, [n.id]: false };
    }
  }
  let reverted = $state<Record<string, string>>({});

  // ── The reader: the full text, and the evidence behind it (what a
  // proposal changes, ProposedChange; a failed run's trail). Fetched once
  // per id. ──
  let copied = $state(false);
  const selected = $derived($notifications.find((n) => n.id === selectedId) ?? $history.find((n) => n.id === selectedId) ?? null);
  type Detail = { kind: 'learning'; data: Learning } | { kind: 'run'; data: GetRunResponse } | { kind: 'gone' };
  let details = $state<Record<string, Detail>>({});
  const detailFetched = new Set<string>();
  $effect(() => {
    const n = selected;
    if (!n || detailFetched.has(n.id)) return;
    const learning = n.type === 'rule_offer' ? null : learningIdOf(n);
    const fetch: Promise<Detail> | null = learning
      ? getLearning(learning).then((r) => ({ kind: 'learning', data: r as Learning }))
      : n.id.startsWith('wf-fail:') && n.agentId
        ? getRun(`agent:${n.agentId}`, n.id.slice('wf-fail:'.length)).then((r) => ({ kind: 'run', data: r }))
        : null;
    if (!fetch) return;
    detailFetched.add(n.id);
    // A 404 is an answer, not a retry: the run row goes when its employee
    // is removed, and the reader says so instead of showing nothing.
    fetch.then((d) => { details = { ...details, [n.id]: d }; }).catch(() => { details = { ...details, [n.id]: { kind: 'gone' } }; });
  });
  const detail = $derived(selected ? details[selected.id] ?? null : null);

  async function copyBody(n: InboxItem) {
    await navigator.clipboard.writeText(n.body ?? '');
    copied = true;
    setTimeout(() => (copied = false), 1500);
  }
</script>

{#snippet card(n: InboxItem)}
  <NeedsYouCard item={n} name={who(n.agentId).name} color={who(n.agentId).color} {assistantName} onopen={onnavigate} />
{/snippet}

<!-- One update: title, time, sentence, its link, a dot while unread. A
     handled one also says why (the rule) and offers Undo where it can be. -->
{#snippet update(n: InboxItem)}
  {@const handled = cardKindOf(n) === 'handled'}
  {@const u = undoOf(n)}
  <div class="inbox-update group">
    <span class="inbox-dot {n.readAt ? '' : 'inbox-dot-unread'}" aria-label={n.readAt ? undefined : $t('inbox.unread')}></span>
    <div class="flex-1 min-w-0">
      <button type="button" class="inbox-update-main" onclick={() => open(n)}>
        <span class="text-sm truncate {n.readAt ? 'font-medium' : 'font-semibold'}">{n.title}</span>
        <span class="text-xs text-base-content/50 font-mono shrink-0 ms-auto">{formatRelative(n.createdAt, 'short')}</span>
      </button>
      {#if n.body}<p class="text-xs text-base-content/70 mt-0.5">{n.body}</p>{/if}
      {#if (n.actionUrl && n.linkLabel) || handled}
        <div class="flex flex-wrap items-center gap-1.5 mt-1.5">
          {#if n.actionUrl && n.linkLabel}
            <button type="button" class="inbox-link" onclick={() => { markAsRead(n.id); if (n.actionUrl) onnavigate(n.actionUrl); }}>↗ {n.linkLabel}</button>
          {/if}
          {#if handled && ruleIdOf(n)}
            <button type="button" class="inbox-link" aria-expanded={whyOpen === n.id} onclick={() => (whyOpen = whyOpen === n.id ? null : n.id)}>{$t('inbox.why')}</button>
          {/if}
          {#if u}
            <button type="button" class="inbox-link" disabled={!!busy[n.id]} onclick={() => undo(n)}><Undo2 class="w-3 h-3" />{u.label}</button>
          {/if}
        </div>
      {/if}
      {#if handled && whyOpen === n.id && ruleIdOf(n)}
        <RuleWhy ruleId={ruleIdOf(n)!} {assistantName} onnavigate={onnavigate} />
      {/if}
    </div>
    <button type="button" class="inbox-put-away" onclick={() => putAway(n.id)} aria-label={$t('inbox.putAwayAction')} title={$t('inbox.putAwayAction')}>
      <Archive class="w-3 h-3" />
    </button>
  </div>
{/snippet}

{#snippet past(n: InboxItem)}
  <div class="inbox-update group">
    <span class="inbox-dot"></span>
    <div class="flex-1 min-w-0">
      <button type="button" class="inbox-update-main" onclick={() => open(n)}>
        <span class="text-xs text-base-content/70 shrink-0">{who(n.agentId).name}</span>
        <span class="text-sm font-medium truncate">{n.title}</span>
        <span class="text-xs text-base-content/50 font-mono shrink-0 ms-auto">{formatRelative(n.answeredAt ?? n.archivedAt ?? n.createdAt, 'short')}</span>
      </button>
      <p class="text-xs text-base-content/70 mt-0.5">
        {#if n.status === 'answered'}
          {$t('inbox.youAnswered', { values: { answer: n.answer ?? '' } })}{#if surfaceKey(n.answeredVia)} · {$t(surfaceKey(n.answeredVia)!)}{/if}
        {:else}
          {$t('inbox.putAway')}
        {/if}
      </p>
      {#if canRevertLearning(n)}
        <div class="mt-1.5">
          {#if reverted[n.id]}
            <span class="text-xs text-base-content/60">{$t(reverted[n.id] === 'conflict' ? 'inbox.conflict' : 'inbox.reverted')}</span>
          {:else}
            <button type="button" class="inbox-link" disabled={!!busy[n.id]} onclick={() => revert(n)}><Undo2 class="w-3 h-3" />{$t('inbox.revert')}</button>
          {/if}
        </div>
      {/if}
    </div>
  </div>
{/snippet}

<div class="flex-1 flex min-h-0 min-w-0 bg-base-100">
  {#if !selected}
    <div class="flex-1 min-w-0 overflow-y-auto">
      <div class="inbox-feed">
        <div class="flex items-center gap-2">
          {#if !embedded}<h1 class="text-base font-semibold">{$t('inbox.title')}</h1>{/if}
          <div class="flex-1 min-w-0 flex items-center h-8 rounded-[5px] px-[9px] gap-1.5 text-sm border border-base-300 bg-base-100">
            <SearchIcon class="w-3 h-3 text-base-content/50 shrink-0" />
            <input type="text" bind:value={search} placeholder={$t('common.search')} class="flex-1 bg-transparent border-none outline-none text-sm placeholder:text-base-content/50 min-w-0" />
            {#if search}
              <button type="button" class="p-0 bg-transparent border-none cursor-pointer shrink-0" onclick={() => (search = '')} aria-label={$t('common.close')}>
                <X class="w-3 h-3 text-base-content/50" />
              </button>
            {/if}
          </div>
          <div class="dropdown dropdown-end shrink-0">
            <button type="button" class="btn btn-ghost btn-sm btn-square" aria-label={$t('inbox.more')} title={$t('inbox.more')}>
              <MoreHorizontal class="w-4 h-4" />
            </button>
            <ul class="dropdown-content menu z-20 mt-1 w-60 rounded-box border border-base-300 bg-base-100 p-1.5 shadow-lg">
              <li><button type="button" onclick={markAllRead}>{$t('notifications.markAllRead')}</button></li>
              <li><button type="button" onclick={() => onnavigate(rulesPath())}>{$t('inbox.whatHandles', { values: { name: assistantName } })}</button></li>
              <li><button type="button" onclick={() => pick('history')}>{$t('inbox.history')}</button></li>
            </ul>
          </div>
        </div>

        <div class="flex items-center gap-1 overflow-x-auto pb-0.5">
          {#each [['needs_you', 'inbox.needsYou'], ['updates', 'inbox.updates'], ['history', 'inbox.history']] as [id, label] (id)}
            <button type="button" class="btn btn-xs whitespace-nowrap {filter === id ? 'btn-neutral' : 'btn-ghost text-base-content/70'}" onclick={() => pick(id as Filter)}>{$t(label)}</button>
          {/each}
          {#each employees as e (e.id)}
            <button type="button" class="btn btn-xs whitespace-nowrap {filter === `agent:${e.id}` ? 'btn-neutral' : 'btn-ghost text-base-content/70'}" onclick={() => pick(`agent:${e.id}`)}>{e.name}</button>
          {/each}
        </div>

        {#if filter === 'history'}
          <section class="flex flex-col">
            <div class="inbox-section-head"><h2>{$t('inbox.history')}</h2></div>
            {#each pastItems as n (n.id)}
              {@render past(n)}
            {:else}
              <p class="text-xs text-base-content/60 py-6 text-center">{$t('inbox.historyEmpty')}</p>
            {/each}
          </section>
        {:else}
          {#if filter === 'all' && !search}
            <div>
              <div class="text-xl font-semibold tracking-tight">{greeting}</div>
              <div class="text-sm text-base-content/70 mt-0.5">{$t('inbox.counts', { values: { needs: $inboxCounts.needsYou, updates: $inboxCounts.unread } })}</div>
            </div>
            {#if brief}
              <div class="inbox-brief" data-testid="inbox-brief">
                <div class="inbox-card-meta">
                  <AgentAvatar name={assistantName || 'N'} color={roster[ASSISTANT_ID]?.color} size="xs" />
                  <span>{$t('inbox.yourAssistant', { values: { name: assistantName } })}</span>
                </div>
                <p class="text-sm text-base-content">{brief.body || brief.title}</p>
                <button type="button" class="inbox-ask" onclick={() => { markAsRead(brief.id); onnavigate('/assistant'); }}>
                  {$t('inbox.askAbout', { values: { name: assistantName } })}
                </button>
              </div>
            {/if}
          {/if}

          {#if filter !== 'updates' && needs.shown.length > 0}
            <section class="flex flex-col gap-2.5">
              <div class="inbox-section-head"><h2>{$t('inbox.needsYou')}</h2><span>{needs.shown.length + needs.more}</span></div>
              {#each needs.shown as n (n.id)}
                {@render card(n)}
              {/each}
              {#if needs.more > 0}
                <button type="button" class="inbox-more" onclick={() => (moreNeeds = true)}>{$t('inbox.showMore', { values: { n: needs.more } })}</button>
              {/if}
            </section>
          {/if}

          {#if filter !== 'needs_you' && groups.length > 0}
            <section class="flex flex-col gap-2.5">
              <div class="inbox-section-head"><h2>{$t('inbox.updates')}</h2><span>{$t('inbox.updatesByEmployee')}</span></div>
              {#each split.shown as g (g.agentId)}
                <div class="inbox-group">
                  <div class="flex items-center gap-2.5 pb-1">
                    <AgentAvatar name={who(g.agentId).name || 'N'} color={who(g.agentId).color} size="sm" />
                    <div class="min-w-0">
                      <div class="text-sm font-medium truncate">{who(g.agentId).name}</div>
                      <div class="text-xs text-base-content/70">
                        {g.handled === g.items.length ? $t('inbox.handledCount', { values: { n: g.handled } }) : $t('inbox.updateCount', { values: { n: g.items.length } })}
                      </div>
                    </div>
                  </div>
                  {#each g.items as n (n.id)}
                    {@render update(n)}
                  {/each}
                </div>
              {/each}
              {#if split.rest.length > 0}
                <button type="button" class="inbox-more" onclick={() => (moreGroups = true)}>
                  <ChevronDown class="w-3.5 h-3.5" />
                  {$t('inbox.moreEmployees', { values: { name: who(split.rest[0].agentId).name, n: split.rest.length - 1 } })}
                </button>
              {/if}
              {#if $hasMore}
                <div bind:this={sentinel} class="flex justify-center py-3">
                  <span class="loading loading-dots loading-xs text-base-content/40"></span>
                </div>
              {/if}
            </section>
          {/if}

          <div class="inbox-caught-up">
            {#if needs.shown.length === 0}<span>{$t('inbox.caughtUp')}</span><span aria-hidden="true">·</span>{/if}
            <button type="button" class="inbox-link" onclick={() => pick('history')}>{$t('inbox.history')}</button>
          </div>
        {/if}
      </div>
    </div>
  {:else}
    <!-- The reader -->
    <div class="flex-1 min-w-0 flex flex-col min-h-0">
      <div class="flex items-center gap-2 h-12 px-4 border-b border-base-content/10 shrink-0">
        <button class="p-1.5 rounded hover:bg-base-200 cursor-pointer bg-transparent border-none" onclick={() => onselect(null)} aria-label={$t('common.back')}>
          <ArrowLeft class="w-4 h-4 text-base-content/70" />
        </button>
        <span class="text-sm font-medium truncate min-w-0">{selected.title}</span>
        <span class="badge badge-ghost badge-sm shrink-0">{who(selected.agentId).name}</span>
        <span class="text-xs text-base-content/50 font-mono shrink-0">{formatRelative(selected.createdAt, 'short')}</span>
        <div class="ms-auto flex items-center gap-1 shrink-0">
          {#if elsewhere(selected.actionUrl)}
            <button class="btn btn-ghost btn-xs gap-1.5" onclick={() => selected?.actionUrl && onnavigate(selected.actionUrl)}>
              <ExternalLink class="w-3.5 h-3.5" />
              {selected.linkLabel || $t('common.open')}
            </button>
          {/if}
          <button class="btn btn-ghost btn-xs gap-1.5" onclick={() => selected && copyBody(selected)}>
            {#if copied}<Check class="w-3.5 h-3.5 text-success" />{:else}<Copy class="w-3.5 h-3.5" />{/if}
            {$t('common.copy')}
          </button>
          {#if sectionOf(selected) !== 'history'}
            <button class="btn btn-ghost btn-xs" onclick={() => selected && putAway(selected.id)} aria-label={$t('inbox.putAwayAction')} title={$t('inbox.putAwayAction')}>
              <Archive class="w-3.5 h-3.5 text-base-content/50" />
            </button>
          {/if}
        </div>
      </div>
      <div class="flex-1 min-h-0 overflow-y-auto">
        <div class="max-w-2xl w-full mx-auto px-6 py-6 flex flex-col gap-6">
          {#if sectionOf(selected) === 'needs_you' || selected.status === 'answered'}
            {@render card(selected)}
          {:else}
            <div class="prose prose-sm max-w-none [&>:first-child]:mt-0">
              {@html parseMarkdown(selected.body ?? '')}
            </div>
          {/if}
          {#if detail?.kind === 'learning'}
            <ProposedChange learning={detail.data} />
          {:else if detail?.kind === 'gone'}
            <div class="text-xs text-base-content/70">{$t('inbox.recordGone')}</div>
          {:else if detail?.kind === 'run'}
            {@const run = detail.data.run}
            {@const steps = Object.values((detail.data.taskItems ?? {}) as Record<string, PendingTask[]>).flat()}
            <div class="space-y-4">
              {#if run.error}
                <div>
                  <div class="text-xs font-semibold uppercase tracking-wide text-base-content/50 mb-2">{run.errorActivity ? $t('inbox.failedAt', { values: { activity: run.errorActivity } }) : $t('inbox.whatHappened')}</div>
                  <pre class="text-xs whitespace-pre-wrap break-words rounded-lg bg-error/10 text-error p-3">{run.error}</pre>
                </div>
              {/if}
              {#if steps.length > 0}
                <div>
                  <div class="text-xs font-semibold uppercase tracking-wide text-base-content/50 mb-2">{$t('inbox.steps')}</div>
                  <ol class="space-y-2">
                    {#each steps as step (step.id)}
                      <li class="text-sm flex items-start gap-2">
                        <span class="badge badge-sm shrink-0 font-mono {step.status === 'failed' ? 'badge-error badge-outline' : step.status === 'completed' ? 'badge-success badge-outline' : 'badge-ghost'}">{step.status}</span>
                        <div class="min-w-0">
                          <div>{step.description || step.prompt}</div>
                          {#if step.lastError}<div class="text-xs text-error/80 mt-0.5">{step.lastError}</div>{/if}
                          {#if step.output}<pre class="text-xs whitespace-pre-wrap break-words rounded-lg bg-base-200 p-2 mt-1">{step.output}</pre>{/if}
                        </div>
                      </li>
                    {/each}
                  </ol>
                </div>
              {/if}
              {#if run.output}
                <div>
                  <div class="text-xs font-semibold uppercase tracking-wide text-base-content/50 mb-2">{$t('inbox.output')}</div>
                  <pre class="text-xs whitespace-pre-wrap break-words rounded-lg bg-base-200 p-3">{run.output}</pre>
                </div>
              {/if}
            </div>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>
