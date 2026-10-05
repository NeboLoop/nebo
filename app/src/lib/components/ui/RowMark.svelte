<!--
  The one mark at the end of a sidebar row (`rowMark`): a pulsing green dot
  while the employee or team works in the conversation (the avatar's own
  working dot; still under reduced motion), a primary dot once a reply waits
  unread. Nothing when neither. Named for screen readers either way.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { RowMark } from '$lib/chat/rowState';

  let { mark }: { mark: RowMark } = $props();
</script>

{#if mark === 'working'}
  <span class="shrink-0 self-center w-2 h-2 rounded-full bg-success agent-working-dot" role="img" aria-label={$t('sidebar.working')} title={$t('sidebar.working')} data-testid="row-working"></span>
{:else if mark === 'unread'}
  <span class="shrink-0 self-center w-2 h-2 rounded-full bg-primary" role="img" aria-label={$t('sidebar.newReply')} title={$t('sidebar.newReply')} data-testid="row-unread"></span>
{/if}
