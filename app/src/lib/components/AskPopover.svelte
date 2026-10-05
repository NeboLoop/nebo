<!--
  AskPopover — a blocking ask's card over whatever screen the owner is on,
  opened only by his click on its toast or OS notification
  (`$lib/stores/blockingAsks`). Small, no backdrop, blocks nothing: he
  answers and stays in the chat he was in. Escape or the close button puts
  him back exactly where he was; "See conversation" is the one way it moves
  him. Answered anywhere else first, it is gone (`asks_waiting`).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { goto } from '$lib/nav';
  import { waitingAsks } from '$lib/stores/waitingAsks';
  import { openAskCard, closeAsk, askHome, answerById } from '$lib/stores/blockingAsks';
  import X from 'lucide-svelte/icons/x';

  const ask = $derived($openAskCard ? $waitingAsks.find((a) => a.id === $openAskCard) : undefined);
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
    const home = askHome(ask);
    closeAsk();
    void goto(home);
  }
</script>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && $openAskCard) closeAsk(); }} />

{#if ask}
  <div
    class="fixed bottom-4 end-4 z-[101] w-80 max-w-[calc(100vw-2rem)] rounded-xl border border-base-300 bg-base-100 shadow-xl p-4 flex flex-col gap-3"
    role="dialog"
    aria-modal="false"
    aria-label={$t('chat.waitingOnYou', { values: { name: ask.employee } })}
    data-testid="ask-popover"
  >
    <div class="flex items-start gap-2">
      <div class="flex-1 min-w-0 text-sm font-semibold text-base-content">
        {$t('chat.waitingOnYou', { values: { name: ask.employee } })}
      </div>
      <button
        type="button"
        class="p-0.5 rounded hover:bg-base-content/10 transition-colors cursor-pointer bg-transparent border-none"
        aria-label={$t('common.close')}
        onclick={closeAsk}
      >
        <X class="w-3.5 h-3.5 text-base-content/50" />
      </button>
    </div>
    <p class="text-sm text-base-content/80 whitespace-pre-line break-words">{ask.question}</p>
    {#if ask.answerable && ask.options.length > 0}
      <div class="flex flex-wrap gap-2">
        {#each ask.options as label, i (label)}
          <button
            type="button"
            class="btn btn-sm {i === 0 ? 'btn-primary' : 'btn-ghost'}"
            disabled={sending}
            onclick={() => answer(i)}
          >{label}</button>
        {/each}
      </div>
    {/if}
    <button
      type="button"
      class="link link-hover text-xs text-base-content/60 self-start bg-transparent border-none p-0 cursor-pointer"
      onclick={seeConversation}
    >{$t('chat.seeConversation')}</button>
  </div>
{/if}
