<!--
  CoworkerThreadView — the view-only transcript of one employee↔employee
  thread (the WS5 audit surface). Opened from a "Messaged {name}" event chip;
  the owner READS what one employee told another — steering an employee
  happens in that employee's own chat, so there is no composer here, ever.

  Data: GET /agent/sessions/{threadKey}/messages — coworker threads are real
  sessions (agent:{B}:coworker:{ctx}), so the transcript is the actual record,
  not a reconstruction.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import { onMount } from 'svelte';
  import { getSessionMessages } from '$lib/api/nebo';
  import type { ChatMessage } from '$lib/api/neboComponents';
  import { parseMarkdown } from '$lib/markdown';
  import TranscriptMessage from '$lib/components/chat/TranscriptMessage.svelte';

  let {
    threadKey,
    senderName,
    targetName,
  }: { threadKey: string; senderName: string; targetName: string } = $props();

  let messages: ChatMessage[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      const resp = await getSessionMessages(encodeURIComponent(threadKey), 200);
      messages = (resp?.messages ?? []).filter(
        (m) => m.role === 'user' || m.role === 'assistant'
      );
    } catch {
      messages = [];
    } finally {
      loading = false;
    }
  });

  // In the target-side thread, "user" rows are the SENDING employee's
  // messages and "assistant" rows are the target employee's replies.
  const nameFor = (role: string) => (role === 'assistant' ? targetName : senderName);

  function timeLabel(epoch: number): string {
    try {
      return new Date(epoch * 1000).toLocaleString([], {
        month: 'short',
        day: 'numeric',
        hour: 'numeric',
        minute: '2-digit',
      });
    } catch {
      return '';
    }
  }
</script>

<div class="flex-1 min-h-0 flex flex-col">
  <div class="flex-1 min-h-0 overflow-y-auto px-5 py-4">
    {#if loading}
      <div class="flex justify-center py-16">
        <span class="loading loading-spinner loading-md text-primary"></span>
      </div>
    {:else if messages.length === 0}
      <div class="flex flex-col items-center justify-center py-16 text-center">
        <p class="text-sm text-base-content/50">{$t('coworkerThread.empty')}</p>
      </div>
    {:else}
      <div class="max-w-2xl mx-auto flex flex-col gap-4" data-selectable>
        {#each messages as m (m.id)}
          <TranscriptMessage
            name={nameFor(m.role)}
            time={timeLabel(m.createdAt)}
            mine={m.role !== 'assistant'}
            html={parseMarkdown(m.content)}
          />
        {/each}
      </div>
    {/if}
  </div>
  <!-- The defining affordance: no composer. This is a record, not a chat. -->
  <div class="shrink-0 border-t border-base-300 px-5 py-3 flex items-center justify-center gap-1.5 text-xs text-base-content/50">
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="11" width="18" height="11" rx="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/></svg>
    <span>{$t('coworkerThread.viewOnly')}</span>
  </div>
</div>
