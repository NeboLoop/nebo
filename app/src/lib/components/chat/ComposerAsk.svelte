<!--
  ComposerAsk — the question or approval waiting in the chat on screen, in
  the composer's place, so it never scrolls out of sight while the employee
  keeps working. One at a time, "1 of 3" when more wait; each answer brings
  the next. The mic answers it by voice, in this chat's call. The X closes
  it without answering: the composer comes back, and
  a chip above it ("1 question waiting") reopens it. The Inbox still has it.
  Each is the ONE decision card (DecisionCard) the chat and the Inbox show,
  on the same answer paths: answered here or there, it leaves everywhere.
  Stop, in the card's corner, stops the employee's work.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import X from 'lucide-svelte/icons/x';
  import AudioLines from 'lucide-svelte/icons/audio-lines';
  import AskWidget from './AskWidget.svelte';
  import ApprovalAskCard from './ApprovalAskCard.svelte';
  import PermissionAskCard from '$lib/components/PermissionAskCard.svelte';
  import type { ComposerAsk } from '$lib/chat/composerAsks';

  let {
    asks,
    onasksubmit,
    ondismiss,
    onvoice,
    asker = null,
    onstop
  }: {
    asks: ComposerAsk[];
    onasksubmit?: (requestId: string, value: string) => void;
    ondismiss: () => void;
    /** The mic: answer it by voice, in this chat's call. */
    onvoice?: () => void;
    /** The chat's employee: who asks its questions. */
    asker?: { id: string; name: string } | null;
    /** Stop the employee's work; absent when nothing runs. */
    onstop?: () => void;
  } = $props();

  const ask = $derived(asks[0]);
</script>

{#if ask}
  <div class="composer-ask ask-rise" data-testid="composer-ask">
    <div class="composer-ask-head">
      <span class="composer-ask-count">{#if asks.length > 1}{$t('chat.askStepOf', { values: { current: 1, total: asks.length } })}{/if}</span>
      {#if onvoice}
        <button type="button" class="composer-ask-close" title={$t('chat.askAnswerByVoice')} aria-label={$t('chat.askAnswerByVoice')} onclick={onvoice}>
          <AudioLines class="w-3.5 h-3.5" />
        </button>
      {/if}
      <button type="button" class="composer-ask-close" title={$t('common.close')} aria-label={$t('common.close')} onclick={ondismiss}>
        <X class="w-3.5 h-3.5" />
      </button>
    </div>
    {#key ask.id}
      {#if ask.kind === 'question'}
        <AskWidget requestId={ask.id} prompt={ask.prompt} widgets={ask.widgets} onSubmit={(id, value) => onasksubmit?.(id, value)} {ondismiss} {asker} {onstop} />
      {:else if ask.kind === 'permission'}
        <PermissionAskCard ask={ask.ask} via="chat" {onstop} />
      {:else}
        <ApprovalAskCard approval={ask.approval} {onstop} />
      {/if}
    {/key}
  </div>
{/if}
