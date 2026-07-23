<!--
  ApprovalGate — the single, app-wide consumer of the `approval_request` WS event.

  The runner pauses a tool call when a capability is OFF (and Full Access is off)
  and emits `approval_request`; this gate shows the ApprovalModal and sends the
  user's decision back via `approval_response`. Mounted once in the root layout
  so it works regardless of which view is open (the previous wiring only raised a
  toast, so the run hung with no way to answer). FIFO queue — one modal at a time.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import ApprovalModal from '$lib/components/ApprovalModal.svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { getWebSocketClient } from '$lib/websocket/client';

  interface PendingApproval {
    requestId: string;
    agent: string;
    actionType: string;
    actionDetail: string;
  }

  let queue = $state<PendingApproval[]>([]);
  const current = $derived(queue[0] ?? null);

  // Map the tool call to the modal's action display.
  function describe(tool: string, input: Record<string, unknown> | undefined): {
    actionType: string;
    actionDetail: string;
  } {
    const action = String(input?.action ?? '');
    const resource = String(input?.resource ?? '');
    const str = (v: unknown) => (typeof v === 'string' ? v : undefined);
    if (resource === 'shell' || action === 'exec') {
      return { actionType: 'shell_command', actionDetail: str(input?.command) ?? '' };
    }
    if (resource === 'file' && (action === 'write' || action === 'edit')) {
      return { actionType: 'file_write', actionDetail: str(input?.path) ?? '' };
    }
    if (tool === 'web') {
      return { actionType: 'http_request', actionDetail: str(input?.url) ?? JSON.stringify(input ?? {}) };
    }
    return {
      actionType: tool || 'action',
      actionDetail:
        str(input?.command) ?? str(input?.path) ?? str(input?.url) ?? JSON.stringify(input ?? {}),
    };
  }

  onWsEvent<{ request_id?: string; agentName?: string; tool?: string; input?: Record<string, unknown> }>(
    'approval_request',
    (d) => {
      if (!d?.request_id) return;
      const { actionType, actionDetail } = describe(d.tool ?? '', d.input);
      queue = [
        ...queue,
        { requestId: d.request_id, agent: d.agentName ?? $t('components.approvalGate.yourAgent'), actionType, actionDetail },
      ];
    }
  );

  function respond(approved: boolean, always: boolean) {
    const req = queue[0];
    if (!req) return;
    getWebSocketClient().send('approval_response', {
      request_id: req.requestId,
      approved,
      always,
    });
    queue = queue.slice(1);
  }
</script>

{#if current}
  {#key current.requestId}
    <ApprovalModal
      show={true}
      agent={current.agent}
      actionType={current.actionType}
      actionDetail={current.actionDetail}
      onApprove={() => respond(true, false)}
      onApproveAlways={() => respond(true, true)}
      onDeny={() => respond(false, false)}
    />
  {/key}
{/if}
