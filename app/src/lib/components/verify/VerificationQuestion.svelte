<!--
  VerificationQuestion — the question the verify loop waits on, in its sheet:
  each choice it offered, Skip, and back to the composer to tell it in his own
  words. A choice is chosen at once and sent once (`onpick`, true when the
  bot took it); one that did not send says so, with Try again.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { SKIP_VALUE } from '$lib/components/chat/AskWidget.svelte';
  import { wordedOption, wordedQuestion, type AskWords } from '$lib/chat/askWords';

  let {
    ask,
    onpick,
    onclose,
  }: {
    ask: { question: string; options: string[]; words?: AskWords };
    onpick: (value: string) => Promise<boolean>;
    onclose: () => void;
  } = $props();

  /** The answer on its way; every tap meanwhile does nothing. */
  let sending = $state<string | null>(null);
  let failed = $state(false);

  async function pick(value: string) {
    if (sending !== null && !failed) return;
    sending = value;
    failed = false;
    failed = !(await onpick(value));
  }
</script>

<section class="verify-question">
  <p class="verify-question-text">{wordedQuestion($t, ask.question, ask.words)}</p>
  <div class="verify-question-choices">
    {#each ask.options as o (o)}
      <button type="button" class="btn btn-sm btn-primary btn-soft {sending === o ? 'verify-question-chosen' : ''}" data-answer={o} disabled={sending !== null && !failed} onclick={() => pick(o)}>{wordedOption($t, ask.options, o, ask.words)}</button>
    {/each}
  </div>
  {#if sending !== null && !failed}
    <p class="verify-question-status"><span class="loading loading-spinner loading-xs" aria-hidden="true"></span>{$t('verify.sending')}</p>
  {:else if failed && sending !== null}
    <div class="verify-question-more">
      <span class="verify-question-failed">{$t('verify.sendFailed')}</span>
      <button type="button" class="btn btn-ghost btn-xs" onclick={() => pick(sending ?? SKIP_VALUE)}>{$t('chat.tryAgain')}</button>
    </div>
  {:else}
    <div class="verify-question-more">
      <button type="button" class="btn btn-ghost btn-xs" onclick={onclose}>{$t('verify.tellIt')}</button>
      <button type="button" class="btn btn-ghost btn-xs" data-answer={SKIP_VALUE} onclick={() => pick(SKIP_VALUE)}>{$t('common.skip')}</button>
    </div>
  {/if}
</section>
