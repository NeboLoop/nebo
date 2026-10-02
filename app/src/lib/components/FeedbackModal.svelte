<!--
  Provide feedback, from the account menu: a message, optional screenshots,
  diagnostics on by default, Send. It goes to the NeboAI team's support inbox
  (see $lib/feedback). A failure keeps everything typed and offers Retry.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { page } from '$app/stores';
  import { tick } from 'svelte';
  import X from 'lucide-svelte/icons/x';
  import ImagePlus from 'lucide-svelte/icons/image-plus';
  import ShelfModal from '$lib/components/ui/ShelfModal.svelte';
  import { FEEDBACK_MAX_ATTACHMENTS, FEEDBACK_MESSAGE_MAX, feedbackProblem, screenOf, sendFeedback } from '$lib/feedback';

  interface Props {
    open: boolean;
    onclose: () => void;
  }
  let { open, onclose }: Props = $props();

  let message = $state('');
  let files = $state<File[]>([]);
  let includeDiagnostics = $state(true);
  let status = $state<'editing' | 'sending' | 'failed' | 'sent'>('editing');
  let messageError = $state('');
  let failure = $state('');
  let textarea = $state<HTMLTextAreaElement | null>(null);
  let picker = $state<HTMLInputElement | null>(null);

  const ids = { message: 'feedback-message', messageError: 'feedback-message-error', diag: 'feedback-diagnostics', diagHelp: 'feedback-diagnostics-help' };

  $effect(() => {
    if (open && status !== 'sent') tick().then(() => textarea?.focus());
  });

  function close() {
    // A sent form starts fresh next time; an unsent one keeps its words.
    if (status === 'sent') {
      message = '';
      files = [];
      includeDiagnostics = true;
      status = 'editing';
    }
    onclose();
  }

  function addFiles(e: Event) {
    const input = e.currentTarget as HTMLInputElement;
    const picked = Array.from(input.files ?? []).filter((f) => f.type.startsWith('image/'));
    files = [...files, ...picked].slice(0, FEEDBACK_MAX_ATTACHMENTS);
    input.value = '';
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (status === 'sending') return;
    const problem = feedbackProblem({ message, files });
    if (problem) {
      messageError = problem;
      textarea?.focus();
      return;
    }
    messageError = '';
    status = 'sending';
    try {
      await sendFeedback({ message, includeDiagnostics, screen: screenOf($page.url), files });
      status = 'sent';
    } catch (err) {
      failure = err instanceof Error && err.message ? err.message : $t('feedback.failed');
      status = 'failed';
    }
  }
</script>

<ShelfModal {open} title={$t('feedback.title')} onclose={close} narrow>
  {#if status === 'sent'}
    <div class="flex-1 flex flex-col items-center justify-center gap-4 p-8 text-center">
      <p class="text-sm font-medium" role="status">{$t('feedback.sent')}</p>
      <button type="button" class="btn btn-primary btn-sm" onclick={close}>{$t('common.done')}</button>
    </div>
  {:else}
    <form class="flex-1 min-h-0 overflow-y-auto flex flex-col gap-4 p-4" onsubmit={submit} novalidate>
      <p class="text-xs text-base-content/70">{$t('feedback.lede')}</p>

      <div class="flex flex-col gap-1">
        <label for={ids.message} class="text-sm font-medium">{$t('feedback.message')}</label>
        <textarea
          id={ids.message}
          bind:this={textarea}
          bind:value={message}
          class="textarea textarea-bordered w-full text-sm min-h-32 {messageError ? 'textarea-error' : ''}"
          maxlength={FEEDBACK_MESSAGE_MAX}
          required
          aria-required="true"
          aria-invalid={messageError ? 'true' : undefined}
          aria-describedby={messageError ? ids.messageError : undefined}
          disabled={status === 'sending'}
          oninput={() => (messageError = '')}
        ></textarea>
        {#if messageError}
          <p id={ids.messageError} class="text-xs text-error" role="alert">{messageError}</p>
        {/if}
      </div>

      <div class="flex flex-col gap-2">
        <span class="text-sm font-medium" id="feedback-files-label">{$t('feedback.screenshots')}</span>
        {#if files.length}
          <ul class="flex flex-col gap-1" aria-labelledby="feedback-files-label">
            {#each files as file, i (file.name + i)}
              <li class="flex items-center gap-2 text-sm rounded-md bg-base-200 px-2 py-1">
                <span class="flex-1 min-w-0 truncate">{file.name}</span>
                <button
                  type="button"
                  class="btn btn-ghost btn-xs btn-square"
                  aria-label={$t('feedback.remove', { values: { name: file.name } })}
                  disabled={status === 'sending'}
                  onclick={() => (files = files.filter((_, j) => j !== i))}
                >
                  <X class="w-3.5 h-3.5" />
                </button>
              </li>
            {/each}
          </ul>
        {/if}
        <input bind:this={picker} type="file" accept="image/*" multiple class="hidden" tabindex="-1" aria-hidden="true" onchange={addFiles} />
        <button
          type="button"
          class="btn btn-sm btn-outline self-start"
          disabled={status === 'sending' || files.length >= FEEDBACK_MAX_ATTACHMENTS}
          onclick={() => picker?.click()}
        >
          <ImagePlus class="w-4 h-4" />
          {$t('feedback.addScreenshot')}
        </button>
      </div>

      <div class="flex items-start gap-3">
        <input
          id={ids.diag}
          type="checkbox"
          role="switch"
          class="toggle toggle-sm toggle-primary shrink-0 mt-0.5"
          bind:checked={includeDiagnostics}
          aria-checked={includeDiagnostics}
          aria-describedby={ids.diagHelp}
          disabled={status === 'sending'}
        />
        <div class="flex flex-col">
          <label for={ids.diag} class="text-sm font-medium">{$t('feedback.diagnostics')}</label>
          <span id={ids.diagHelp} class="text-xs text-base-content/70">{$t('feedback.diagnosticsHelp')}</span>
        </div>
      </div>

      {#if status === 'failed'}
        <p class="text-xs text-error" role="alert">{failure}</p>
      {/if}

      <div class="flex justify-end gap-2">
        <button type="button" class="btn btn-ghost btn-sm" onclick={close} disabled={status === 'sending'}>{$t('common.cancel')}</button>
        <button type="submit" class="btn btn-primary btn-sm" disabled={status === 'sending'}>
          {#if status === 'sending'}
            <span class="loading loading-spinner loading-xs" aria-hidden="true"></span>
            {$t('feedback.sending')}
          {:else if status === 'failed'}
            {$t('common.retry')}
          {:else}
            {$t('feedback.send')}
          {/if}
        </button>
      </div>
    </form>
  {/if}
</ShelfModal>
