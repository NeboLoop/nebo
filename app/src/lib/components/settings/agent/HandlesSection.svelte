<!--
  What the assistant handles for the owner, on its own Permissions page:
  every rule he said yes to, each with an on/off switch, how often it was
  used, when and where he said yes, its history, and Edit (the ceiling).
  A rule that acts on something that can't be undone says so. Below: the
  kinds he said never to offer again, and "Add a rule yourself" from his
  own recent answers. Nothing here is deleted: a rule switched off keeps
  its history.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t, locale } from 'svelte-i18n';
  import AgentAvatar from '$lib/components/AgentAvatar.svelte';
  import { botName, loadBotName } from '$lib/stores/botName';
  import { addToast } from '$lib/stores/toast';
  import { tick } from 'svelte';
  import { goto } from '$lib/nav';
  import { formatRelative } from '$lib/time';
  import UpdateRow from '$lib/components/inbox/UpdateRow.svelte';
  import type { InboxItem } from '$lib/inbox/item';
  import { surfaceKey } from '$lib/inbox/item';
  import {
    learnedRules, loadLearnedRules, handledUnder, setRulePaused, editRule, addRule, offerAgain, formatMoment, dollars, cents,
    type LearnedRule, type RuleEvent,
  } from '$lib/inbox/rules';

  let {
    id = '',
    name,
    color = null,
    shape = null,
    highlight = null
  }: {
    /** The assistant's id. */
    id?: string;
    name: string;
    color?: string | null;
    shape?: string | null;
    /** The rule the Inbox's Why? came from: shown and marked. */
    highlight?: string | null;
  } = $props();

  let failed = $state(false);
  onMount(() => {
    void loadBotName();
    loadLearnedRules().catch(() => (failed = true));
  });

  const data = $derived($learnedRules);
  let busy = $state<string | null>(null);
  let editing = $state<string | null>(null);
  let ceiling = $state('');
  let adding = $state(false);
  let picked = $state<string | null>(null);

  async function act(key: string, fn: () => Promise<void>) {
    if (busy) return;
    busy = key;
    try {
      await fn();
    } catch (e) {
      addToast((e as Error)?.message || $t('inbox.answerFailed'), 'error');
    } finally {
      busy = null;
    }
  }

  function startEdit(r: LearnedRule) {
    editing = r.id;
    ceiling = dollars(r.ceilingCents);
  }

  const moment = (c: RuleEvent) => formatMoment(c, $locale ?? undefined);
  const eventKey: Record<string, string> = {
    turned_on: 'handles.eventTurnedOn',
    edited: 'handles.eventEdited',
    paused: 'handles.eventPaused',
    resumed: 'handles.eventResumed',
  };
  // Opened at a rule (the Inbox's Why?): once the list is in, that rule
  // comes into view, marked.
  let section = $state<HTMLElement | null>(null);
  let shown = '';
  $effect(() => {
    const target = highlight ?? '';
    if (!data || !section || shown === target) return;
    shown = target;
    void tick().then(() => {
      const el = (target && section?.querySelector(`[data-rule-id="${CSS.escape(target)}"]`)) || section;
      el?.scrollIntoView({ block: 'start', behavior: 'smooth' });
    });
  });

  // "See what <name> did": what each rule handled, newest first.
  let did = $state<Record<string, { items: InboxItem[]; total: number } | 'loading'>>({});
  async function toggleDid(id: string) {
    if (did[id]) {
      const { [id]: _, ...rest } = did;
      did = rest;
      return;
    }
    did = { ...did, [id]: 'loading' };
    try {
      did = { ...did, [id]: await handledUnder(id) };
    } catch (e) {
      const { [id]: _, ...rest } = did;
      did = rest;
      addToast((e as Error)?.message || $t('handles.loadFailed'), 'error');
    }
  }
  const openItem = (n: InboxItem) => { if (n.actionUrl) void goto(n.actionUrl); };

  const pausedAt = (r: LearnedRule): RuleEvent | undefined => [...r.history].reverse().find((e) => e.event === 'paused');
</script>

<div class="flex items-start gap-3" data-testid="handles-section" bind:this={section}>
  <AgentAvatar {id} {name} {color} {shape} size="md" />
  <div class="min-w-0">
    <div class="text-sm font-semibold">{$t('handles.title', { values: { name } })}</div>
    <div class="text-xs text-base-content/70">{$t('handles.blurb', { values: { bot: data?.rules[0]?.bot || $botName || name } })}</div>
  </div>
</div>

{#if failed && !data}
  <p class="text-xs text-base-content/70">{$t('handles.loadFailed')}</p>
{:else if !data}
  <span class="loading loading-dots loading-xs text-base-content/40"></span>
{:else}
  {#if data.rules.length === 0}
    <p class="text-xs text-base-content/70">{$t('handles.none', { values: { name } })}</p>
  {/if}
  {#each data.rules as r (r.id)}
    <div class="rule-card {r.paused ? 'rule-card-paused' : ''} {highlight === r.id ? 'rule-card-highlight' : ''}" data-rule-id={r.id}>
      <div class="flex items-start gap-3">
        <div class="text-sm font-semibold flex-1 min-w-0">{r.covers}</div>
        <input
          type="checkbox"
          class="toggle toggle-sm toggle-success shrink-0"
          checked={!r.paused}
          disabled={busy === r.id}
          aria-label={$t('handles.onOff')}
          onchange={() => act(r.id, () => setRulePaused(r.id, !r.paused))}
        />
      </div>
      {#if r.does}<p class="text-sm text-base-content/80">{r.does}</p>{/if}
      {#if r.cantUndo}<p class="inbox-cant-undo">{$t('inbox.cantUndo')}</p>{/if}
      <p class="rule-meta">
        <b>{$t('handles.youSaidYes')}</b> {moment(r.consent)}{#if surfaceKey(r.consent.via)}{' · '}{$t(surfaceKey(r.consent.via)!)}{/if}
      </p>
      {#if r.paused && pausedAt(r)}
        <p class="rule-meta"><b>{$t('handles.youPaused')}</b> {moment(pausedAt(r)!)}</p>
      {/if}
      <p class="rule-meta">
        {$t('handles.used', { values: { n: r.uses } })}{#if r.lastUsedAt}{' · '}{$t('handles.lastUsed', { values: { when: formatRelative(r.lastUsedAt) } })}{/if}
      </p>
      {#if r.history.length > 0}
        <ol class="rule-history">
          {#each r.history as e, i (i)}
            <li>
              <span class="text-base-content/50">{moment(e)}</span>
              <span>{$t(eventKey[e.event] ?? 'handles.eventEdited', { values: { name } })}{#if e.detail}: {e.detail}{/if}</span>
            </li>
          {/each}
        </ol>
      {/if}
      {#if editing === r.id}
        <form class="flex flex-wrap items-center gap-2" onsubmit={(e) => { e.preventDefault(); void act(r.id, async () => { await editRule(r.id, cents(ceiling)); editing = null; }); }}>
          <label class="text-xs text-base-content/70" for="ceiling-{r.id}">{$t('handles.ceiling')}</label>
          <input id="ceiling-{r.id}" class="input input-sm w-28" inputmode="decimal" bind:value={ceiling} placeholder={$t('handles.noCeiling')} />
          <button type="submit" class="btn btn-primary btn-xs" disabled={busy === r.id}>{$t('common.save')}</button>
          <button type="button" class="btn btn-ghost btn-xs" onclick={() => (editing = null)}>{$t('common.cancel')}</button>
        </form>
      {:else}
        <div class="flex flex-wrap gap-1.5">
          <button type="button" class="inbox-link" onclick={() => startEdit(r)}>{$t('handles.edit')}</button>
          <button type="button" class="inbox-link" aria-expanded={!!did[r.id]} onclick={() => toggleDid(r.id)}>{$t('handles.seeWhatDid', { values: { name } })}</button>
        </div>
      {/if}
      {#if did[r.id] === 'loading'}
        <span class="loading loading-dots loading-xs text-base-content/40"></span>
      {:else if did[r.id]}
        {@const list = did[r.id] as { items: InboxItem[]; total: number }}
        <div class="rule-did">
          {#each list.items as n (n.id)}
            <UpdateRow item={n} assistantName={name} why={false} onopen={openItem} onnavigate={(link) => void goto(link)} />
          {:else}
            <p class="rule-meta">{$t('handles.didNothing', { values: { name } })}</p>
          {/each}
          {#if list.total > list.items.length}
            <p class="rule-meta">{$t('handles.didMore', { values: { n: list.total - list.items.length } })}</p>
          {/if}
        </div>
      {/if}
    </div>
  {/each}

  <div class="flex flex-col gap-2">
    <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('handles.neverOffer')} · {data.neverOffer.length}</div>
    {#each data.neverOffer as n (n.id)}
      <div class="flex items-center gap-2 text-sm">
        <span class="flex-1 min-w-0 truncate">{n.what}</span>
        <span class="text-xs text-base-content/60 truncate">{n.employee}</span>
        <button type="button" class="inbox-link" disabled={busy === n.id} onclick={() => act(n.id, () => offerAgain(n.id))}>{$t('handles.offerAgain')}</button>
      </div>
    {:else}
      <p class="text-xs text-base-content/70">{$t('handles.neverNone')}</p>
    {/each}
  </div>

  <div class="flex flex-col gap-2">
    {#if !adding}
      <div><button type="button" class="btn btn-sm btn-ghost border border-base-content/15" onclick={() => (adding = true)}>+ {$t('handles.addYourself')}</button></div>
    {:else}
      <div class="text-xs font-semibold uppercase tracking-wider text-base-content/50">{$t('handles.addYourself')}</div>
      {#each data.candidates as c (c.id)}
        <label class="rule-card cursor-pointer {picked === c.id ? 'rule-card-picked' : ''}">
          <span class="flex items-start gap-2">
            <input type="radio" class="radio radio-sm mt-0.5" name="rule-candidate" value={c.id} bind:group={picked} />
            <span class="flex flex-col gap-0.5 min-w-0">
              <span class="text-sm font-semibold">{c.covers}</span>
              {#if c.does}<span class="text-xs text-base-content/70">{c.does}</span>{/if}
              {#if c.stillAsks}<span class="text-xs text-base-content/70">{$t('inbox.ruleStillAsks')}: {c.stillAsks}</span>{/if}
              {#if c.cantUndo}<span class="inbox-cant-undo">{$t('inbox.cantUndo')}</span>{/if}
            </span>
          </span>
        </label>
      {:else}
        <p class="text-xs text-base-content/70">{$t('handles.noCandidates')}</p>
      {/each}
      <div class="flex gap-2">
        <button
          type="button"
          class="btn btn-primary btn-sm"
          disabled={!picked || busy === 'add'}
          onclick={() => act('add', async () => {
            const c = data.candidates.find((x) => x.id === picked);
            if (!c) return;
            await addRule(c.id, c.ceilingCents ?? null);
            adding = false;
            picked = null;
          })}
        >{$t('handles.turnOn')}</button>
        <button type="button" class="btn btn-ghost btn-sm" onclick={() => { adding = false; picked = null; }}>{$t('common.cancel')}</button>
      </div>
    {/if}
  </div>
{/if}
