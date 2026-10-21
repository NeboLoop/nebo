<!--
  VerificationQuestion — the question the verify loop waits on, in its sheet:
  each choice it offered, Skip, and back to the composer to tell it in his own
  words. A choice or Skip answers the question (`onpick`).
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { SKIP_VALUE } from '$lib/components/chat/AskWidget.svelte';

  let {
    ask,
    onpick,
    onclose,
  }: {
    ask: { question: string; options: string[] };
    onpick: (value: string) => void;
    onclose: () => void;
  } = $props();
</script>

<section class="verify-question">
  <p class="verify-question-text">{ask.question}</p>
  <div class="verify-question-choices">
    {#each ask.options as o (o)}
      <button type="button" class="btn btn-sm btn-primary btn-soft" data-answer={o} onclick={() => onpick(o)}>{o}</button>
    {/each}
  </div>
  <div class="verify-question-more">
    <button type="button" class="btn btn-ghost btn-xs" onclick={onclose}>{$t('verify.tellIt')}</button>
    <button type="button" class="btn btn-ghost btn-xs" data-answer={SKIP_VALUE} onclick={() => onpick(SKIP_VALUE)}>{$t('common.skip')}</button>
  </div>
</section>
