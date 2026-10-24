<!--
  AskPopover — a blocking ask's card over whatever screen the owner is on,
  opened only by his click on its toast or OS notification
  (`$lib/stores/blockingAsks`), for an ask from a chat he asked in. A
  relayed ask opens in the assistant's chat instead. Small, no backdrop, blocks nothing: he
  answers and stays in the chat he was in. Escape or the close button puts
  him back exactly where he was; "See conversation" is the one way it moves
  him. Answered anywhere else first, it is gone (`asks_waiting`). Its card is
  the ONE decision card (DecisionCard): each answer a short label with its
  one-line description, the first filled.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { goto } from '$lib/nav';
  import { waitingAsks, askPath } from '$lib/stores/waitingAsks';
  import { openAskCard, closeAsk, answerById } from '$lib/stores/blockingAsks';
  import X from 'lucide-svelte/icons/x';
  import DecisionCard from '$lib/components/DecisionCard.svelte';
  import { relayedCard } from '$lib/components/chat/RelayedAsks.svelte';
  import { normalizeOptions } from '$lib/chat/askWizard';
  import { emphasized, type DecisionChoice } from '$lib/chat/decision';

  const ask = $derived($openAskCard ? $waitingAsks.find((a) => a.id === $openAskCard) : undefined);
  /** Its answers, by their place in the ask's options: each label with the
   *  description its card carries. */
  const choices = $derived.by((): DecisionChoice[] => {
    if (!ask?.answerable) return [];
    const described = normalizeOptions(relayedCard(ask)[0]?.options);
    return ask.options.map((label, i) => ({
      id: String(i),
      label,
      description: described.find((o) => o.label === label)?.description,
      tone: i === 0 ? 'filled' : 'outlined'
    }));
  });
  let sending = $state(false);

  async function answer(i: number) {
    if (!ask || sending) return;
    sending = true;
    try {
      await answerById(ask.id, i);
      closeAsk();
    } finally {
      sending = false;
    }
  }

  function seeConversation() {
    if (!ask) return;
    const home = askPath(ask);
    closeAsk();
    void goto(home);
  }
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && $openAskCard) closeAsk(); }} />

{#if ask}
  <div
    class="ask-popover"
    role="dialog"
    aria-modal="false"
    aria-label={$t('chat.waitingOnYou', { values: { name: ask.employee } })}
    data-testid="ask-popover"
  >
    <button type="button" class="ask-popover-close" aria-label={$t('common.close')} onclick={closeAsk}>
      <X class="w-3.5 h-3.5" />
    </button>
    <DecisionCard
      who={{ id: ask.agentId, name: ask.employee }}
      line={emphasized($t, 'chat.waitingOnYou', { name: ask.employee })}
      reason={ask.question}
      {choices}
      busy={sending}
      onchoose={(id) => answer(Number(id))}
    >
      <button type="button" class="ask-popover-link" onclick={seeConversation}>{$t('chat.seeConversation')}</button>
    </DecisionCard>
  </div>
{/if}
