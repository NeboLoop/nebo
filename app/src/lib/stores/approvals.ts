// Approvals a run waits on (`approval_request`): a gated call, a suggested
// goal, a coworker's call that needs the owner's OK. Each is a card in the
// chat whose own conversation raised it, at the bottom beside the composer,
// never a dialog over the screen (owner, 2026-10-01: "never block a
// screen"). The first answer anywhere settles it (`approval_resolved`), and
// the card collapses in place to its receipt.

import { writable } from 'svelte/store';
import { getWebSocketClient } from '$lib/websocket/client';

export interface ApprovalRow {
  label: string;
  value: string;
}

export interface Approval {
  requestId: string;
  /** The session that asked: the chat it shows in. */
  sessionId: string;
  agent: string;
  actionType: string;
  actionDetail: string;
  headline?: string;
  detailRows?: ApprovalRow[];
  /** once | always | deny, once decided anywhere. */
  decision?: string;
}

/** Oldest first. */
export const approvals = writable<Approval[]>([]);

/** Requests already settled somewhere, so a late card never shows open. */
const settled = new Map<string, string>();

export function approvalRaised(a: Approval): void {
  const decision = settled.get(a.requestId);
  approvals.update((list) =>
    list.some((x) => x.requestId === a.requestId) ? list : [...list, decision ? { ...a, decision } : a]
  );
}

/** Settled anywhere: here, another client, a loop reply. */
export function approvalSettled(requestId: string, decision: string): void {
  settled.set(requestId, decision);
  approvals.update((list) => list.map((a) => (a.requestId === requestId ? { ...a, decision } : a)));
}

/** The owner's answer from the card. */
export function answerApproval(requestId: string, decision: 'once' | 'always' | 'deny'): void {
  getWebSocketClient().send('approval_response', {
    request_id: requestId,
    approved: decision !== 'deny',
    always: decision === 'always',
  });
  approvalSettled(requestId, decision);
}

/** The approvals the chat on `sessionKey` raised. */
export function chatApprovalsOf(list: Approval[], sessionKey: string): Approval[] {
  return sessionKey ? list.filter((a) => a.sessionId === sessionKey) : [];
}
