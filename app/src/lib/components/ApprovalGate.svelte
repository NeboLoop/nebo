<!--
  ApprovalGate — the single, app-wide consumer of the `approval_request` WS event.

  The runner pauses a tool call when a capability is OFF (and Full Access is off)
  or a gated interface operation needs approval, and emits `approval_request`;
  this gate shows the ApprovalModal and sends the user's decision back via
  `approval_response`. Mounted once in the root layout so it works regardless of
  which view is open. FIFO queue — one modal at a time.

  Rendering rule: the modal must read like a sentence to a non-technical owner.
  Gated operations carry a model-written `display` headline (real names, real
  amounts); the fact rows below it are computed deterministically from the actual
  call arguments (cents → dollars, camelCase → words) so the headline is always
  cross-checked by the args. Raw ids and JSON live only in the technical
  disclosure.
-->
<script lang="ts">
  import { t } from 'svelte-i18n';
  import ApprovalModal from '$lib/components/ApprovalModal.svelte';
  import { onWsEvent } from '$lib/websocket/subscribe';
  import { getWebSocketClient } from '$lib/websocket/client';
  import * as api from '$lib/api/nebo';

  interface DetailRow {
    label: string;
    value: string;
  }

  interface PendingApproval {
    requestId: string;
    agent: string;
    actionType: string;
    actionDetail: string;
    headline?: string;
    detailRows?: DetailRow[];
  }

  let queue = $state<PendingApproval[]>([]);
  const current = $derived(queue[0] ?? null);

  // Agent display names, resolved once per session (the runner event carries the
  // session key `agent:<id>:...`, not a name).
  let agentNames: Record<string, string> | null = null;
  async function resolveAgentName(sessionId: string | undefined): Promise<string | null> {
    const m = /^agent:([^:]+):/.exec(sessionId ?? '');
    if (!m) return null;
    if (!agentNames) {
      try {
        const resp = (await api.listAgents()) as { agents?: { id: string; name: string }[] };
        agentNames = Object.fromEntries((resp.agents ?? []).map((a) => [a.id, a.name]));
      } catch {
        agentNames = {};
      }
    }
    return agentNames[m[1]] ?? null;
  }

  // ── Humanizing helpers (deterministic — computed from the real args) ──
  import { operationLabel } from '$lib/utils/operationLabels';

  /** camelCase → "Title Case"; trailing Id/Key dropped ("vendorId" → "Vendor"). */
  function fieldLabel(key: string): string {
    const spaced = key.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/[_-]/g, ' ');
    const words = spaced.split(' ').filter(Boolean);
    if (words.length > 1 && /^(id|key)$/i.test(words[words.length - 1])) words.pop();
    return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
  }

  /** Deterministic fact rows from the typed args. Id-ish fields are excluded —
      they belong in the technical disclosure, not in front of the owner. */
  function factRows(input: Record<string, unknown> | undefined): DetailRow[] {
    if (!input) return [];
    const rows: DetailRow[] = [];
    for (const [key, raw] of Object.entries(input)) {
      if (/(Id|Key)$/.test(key)) continue;
      let label = fieldLabel(key);
      let value: string;
      if (/Cents$/.test(key) && typeof raw === 'number') {
        label = fieldLabel(key.replace(/Cents$/, ''));
        value = `$${(raw / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
      } else if (typeof raw === 'boolean') {
        value = raw ? $t('components.approvalModal.yes') : $t('components.approvalModal.no');
      } else if (raw === null || raw === undefined) {
        continue;
      } else if (typeof raw === 'object') {
        continue; // nested structures stay in the technical disclosure
      } else {
        value = String(raw);
      }
      rows.push({ label, value });
    }
    return rows;
  }

  // Map the tool call to the modal's action display.
  function describe(
    tool: string,
    input: Record<string, unknown> | undefined
  ): Omit<PendingApproval, 'requestId' | 'agent'> {
    const action = String(input?.action ?? '');
    const resource = String(input?.resource ?? '');
    const operation = String(input?.operation ?? '');
    const str = (v: unknown) => (typeof v === 'string' ? v : undefined);

    // Typed gated operation — headline from `display`, facts from the args.
    if (tool === 'plugin' && operation) {
      return {
        actionType: operationLabel(operation),
        actionDetail: JSON.stringify({ operation, input: input?.input ?? {} }),
        headline: str(input?.display),
        detailRows: factRows(input?.input as Record<string, unknown> | undefined),
      };
    }
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

  onWsEvent<{
    request_id?: string;
    agentName?: string;
    session_id?: string;
    tool?: string;
    input?: Record<string, unknown>;
  }>('approval_request', async (d) => {
    if (!d?.request_id) return;
    const described = describe(d.tool ?? '', d.input);
    const agent =
      d.agentName ??
      (await resolveAgentName(d.session_id)) ??
      $t('components.approvalGate.yourAgent');
    queue = [...queue, { requestId: d.request_id, agent, ...described }];
  });

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
      headline={current.headline}
      detailRows={current.detailRows}
      onApprove={() => respond(true, false)}
      onApproveAlways={() => respond(true, true)}
      onDeny={() => respond(false, false)}
    />
  {/key}
{/if}
