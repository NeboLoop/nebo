<!--
  RelayedAsks — in the assistant's chat, what waits on the owner from work
  he is not in (a colleague's hand-off, a schedule, a background run): each
  item names the employee who asked, shows that employee's own card, and
  links to the employee's thread. It is the same card on the same answer
  path (a question's `ask_response`, a permission ask's answer, an
  approval's decision): answering it here answers the employee, and the
  first answer anywhere settles it everywhere. A connect card connects the
  employee it names (its own `agentId`), never the assistant. The assistant
  only relays: none of this is part of its conversation, so it never
  answers a card. A card from a chat the owner asked in stays in that chat
  and is not shown here.
-->
<script lang="ts" module>
  import type { WaitingAsk } from '$lib/api/neboComponents';
  import type { AskWidgetDef } from './AskWidget.svelte';

  /** The parked question's own card, exactly as its conversation shows it. */
  export function relayedCard(ask: WaitingAsk): AskWidgetDef[] {
    const widgets = ask.widgets as AskWidgetDef[] | undefined;
    if (Array.isArray(widgets) && widgets.length > 0) return widgets;
    return [{ type: 'options', multiSelect: false, options: ask.options }];
  }
</script>

<script lang="ts">
  import { t } from 'svelte-i18n';
  import { goto } from '$lib/nav';
  import { waitingAsks, relayedAsks, askPath } from '$lib/stores/waitingAsks';
  import { openAsks } from '$lib/stores/permissionAsks';
  import { approvals, relayedApprovalsOf } from '$lib/stores/approvals';
  import { getWebSocketClient } from '$lib/websocket/client';
  import AskWidget from './AskWidget.svelte';
  import ApprovalAskCard from './ApprovalAskCard.svelte';
  import PermissionAskCard from '$lib/components/PermissionAskCard.svelte';

  const asks = $derived(relayedAsks($waitingAsks));
  const relayedApprovals = $derived(relayedApprovalsOf($approvals));

  /** "Ecommerce Store Manager wants to connect Shopify", or who is waiting. */
  function headline(ask: WaitingAsk): string {
    const w = relayedCard(ask)[0];
    if (w.type === 'connect_account') {
      return $t('chat.relayWantsToConnect', { values: { name: ask.employee, plugin: w.label || w.plugin || '' } });
    }
    return $t('chat.waitingOnYou', { values: { name: ask.employee } });
  }

  function answer(requestId: string, value: string) {
    getWebSocketClient().send('ask_response', { request_id: requestId, value });
  }
</script>

{#each asks as ask (ask.id)}
  {@const permission = ask.kind === 'permission' ? $openAsks.find((a) => a.id === ask.id) : undefined}
  {#if ask.kind === 'question' || permission}
    <div class="relay-item ask-rise" data-testid="relayed-ask">
      <div class="relay-item-head">{headline(ask)}</div>
      {#if permission}
        <PermissionAskCard ask={permission} via="chat" />
      {:else}
        <AskWidget requestId={ask.id} prompt={ask.question} widgets={relayedCard(ask)} onSubmit={answer} />
      {/if}
      <button type="button" class="relay-item-link" onclick={() => goto(askPath(ask))}>
        {$t('chat.relayViewIn', { values: { name: ask.employee } })}
      </button>
    </div>
  {/if}
{/each}
{#each relayedApprovals as approval (approval.requestId)}
  {@const agentId = /^agent:([^:]+):/.exec(approval.sessionId)?.[1]}
  <div class="relay-item ask-rise" data-testid="relayed-approval">
    <ApprovalAskCard {approval} />
    {#if agentId}
      <button type="button" class="relay-item-link" onclick={() => goto(`/${agentId}`)}>
        {$t('chat.relayViewIn', { values: { name: approval.agent } })}
      </button>
    {/if}
  </div>
{/each}
