// Verified code work: the badge on the reply that reported it, and every
// round of its history (`GET /verifications`). A conversation's are read
// once and kept current by `verification_updated`.

import { writable } from "svelte/store";
import type { VerificationView } from "$lib/api/neboComponents";
import { getWebSocketClient } from "$lib/websocket/client";

/** Every verification this screen has read, by id. */
export const verifications = writable<Record<string, VerificationView>>({});

let listening = false;
const read = new Set<string>();

function put(rows: VerificationView[]): void {
  if (!rows.length) return;
  verifications.update((all) => {
    const next = { ...all };
    for (const v of rows) next[v.id] = v;
    return next;
  });
}

/** Keep every verification this screen read current. */
export function followVerifications(): void {
  if (listening) return;
  listening = true;
  getWebSocketClient().on("verification_updated", (v: VerificationView) => {
    if (v?.id) put([v]);
  });
}

/** Read the verifications of conversation `sessionKey` (once per screen). */
export async function trackConversation(sessionKey: string): Promise<void> {
  followVerifications();
  if (!sessionKey || read.has(sessionKey)) return;
  read.add(sessionKey);
  try {
    const { listVerifications } = await import("$lib/api/nebo");
    const res = await listVerifications(undefined, sessionKey);
    put(res?.verifications ?? []);
  } catch {
    read.delete(sessionKey);
  }
}

/** The verifications whose badge sits on message `messageId`. */
export function onMessage(
  all: Record<string, VerificationView>,
  messageId: string,
): VerificationView[] {
  return Object.values(all).filter((v) => v.messageId === messageId);
}

/** The verifications of a conversation still being checked or waiting on
 *  the owner (they have no reply to sit on yet). */
export function inProgress(
  all: Record<string, VerificationView>,
  sessionKey: string,
): VerificationView[] {
  return Object.values(all).filter(
    (v) =>
      v.sessionKey === sessionKey &&
      !v.messageId &&
      (v.badge.state === "checking" || v.badge.state === "waiting"),
  );
}

/** The words of a badge: its label and the one sentence under it, never
 *  more than the record holds. `tr` is the caller's translator. */
export function badgeWords(
  v: VerificationView,
  tr: (
    key: string,
    opts?: { values: Record<string, string | number> },
  ) => string,
): { label: string; detail: string } {
  const b = v.badge;
  const lenses = b.lenses.map((l) => tr(`verify.lens.${l}`)).join(", ");
  switch (b.state) {
    case "verified": {
      const parts = [
        tr("verify.tip.verified", {
          values: { reviewers: b.reviewers, lenses, rounds: b.rounds },
        }),
        tr("verify.tip.checksPassed", {
          values: { checks: b.checks.join(", ") },
        }),
        tr("verify.tip.noneOpen"),
      ];
      if (b.deferred > 0)
        parts.push(
          tr("verify.tip.deferred", { values: { count: b.deferred } }),
        );
      parts.push(
        b.landed
          ? tr("verify.tip.landed")
          : b.reason
            ? b.reason + "."
            : tr("verify.tip.notLanded"),
      );
      parts.push(tr("verify.tip.limit"));
      return { label: tr("verify.badge.verified"), detail: parts.join(" ") };
    }
    case "checking":
      return {
        label: tr("verify.badge.checking"),
        detail: tr("verify.tip.checking"),
      };
    case "waiting":
      return {
        label: tr("verify.badge.waiting"),
        detail: tr("verify.tip.waiting"),
      };
    default: {
      const parts = [b.reason ? b.reason + "." : tr("verify.tip.notVerified")];
      if (b.openBlockers + b.openMajors > 0) {
        parts.push(
          tr("verify.tip.open", {
            values: {
              blockers: b.openBlockers,
              majors: b.openMajors,
              rounds: b.rounds,
            },
          }),
        );
      }
      parts.push(
        b.landed ? tr("verify.tip.landedAnyway") : tr("verify.tip.notLanded"),
      );
      return { label: tr("verify.badge.notVerified"), detail: parts.join(" ") };
    }
  }
}

/** A cost in microcents as the owner reads it. */
export function cost(microcents: number): string {
  const dollars = microcents / 100_000_000;
  if (dollars === 0) return "$0";
  if (dollars < 0.01) return "<$0.01";
  return "$" + dollars.toFixed(2);
}
