// Verified code work: the badge on the reply that reported it, and every
// round of its history (`GET /verifications`). A conversation's are read
// when it opens and again whenever the socket comes back (a change while it
// was down is only in the list), and kept current by `verification_updated`.

import { writable } from "svelte/store";
import type { VerificationView } from "$lib/api/neboComponents";
import { getWebSocketClient } from "$lib/websocket/client";

/** Every verification this screen has read, by id. */
export const verifications = writable<Record<string, VerificationView>>({});

let listening = false;
const read = new Set<string>();

/** What the bot says about a verification or one of its steps: a code the
 *  app words in the owner's language, with its values. */
export type Why = { code: string; args?: Record<string, string | number> };

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
  const ws = getWebSocketClient();
  ws.on("verification_updated", (v: VerificationView) => {
    if (v?.id) put([v]);
  });
  let wasDown = false;
  ws.onStatus((status) => {
    if (status !== "connected") {
      wasDown = true;
      return;
    }
    if (!wasDown) return;
    wasDown = false;
    for (const sessionKey of read) void readConversation(sessionKey);
  });
}

async function readConversation(sessionKey: string): Promise<void> {
  try {
    const { listVerifications } = await import("$lib/api/nebo");
    const res = await listVerifications(undefined, sessionKey);
    put(res?.verifications ?? []);
  } catch {
    read.delete(sessionKey);
  }
}

/** Read the verifications of conversation `sessionKey` (once per screen,
 *  and again after the socket comes back). */
export async function trackConversation(sessionKey: string): Promise<void> {
  followVerifications();
  if (!sessionKey || read.has(sessionKey)) return;
  read.add(sessionKey);
  await readConversation(sessionKey);
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

type Tr = (
  key: string,
  opts?: { values: Record<string, string | number> },
) => string;

/** The codes the bot sends that this app words; any other (a newer bot's)
 *  reads as the generic line, never as its raw code. */
const WHYS = new Set([
  "needs_answer", "unreadable", "spend_limit", "review_incomplete", "needs_decision",
  "stopped_by_owner", "stalled", "no_checks", "landed_unreviewed", "edits_in_way",
  "kept_own_edits", "live_failed", "live_unavailable", "live_ok", "review_clean", "review_blocking",
  "up_to_date", "caught_up", "conflicts", "landed", "nothing_to_land", "owner_edits",
  "busy_no_branch", "busy_branch", "busy_in_progress", "hook_failed", "retest_failed",
  "not_attempted", "land_failed", "step_limit", "turn_ended", "restarted",
  "unreviewed", "needs_review", "changed_after_review", "waiting_to_land",
]);
const LENSES = new Set(["correctness", "security", "billing", "data", "performance", "experience"]);
const SEVERITIES = new Set(["blocker", "major", "minor"]);
const KINDS = new Set(["catch_up", "land", "live"]);

/** What the bot said, in the owner's words. */
export function whyWords(why: Why | null | undefined, tr: Tr): string {
  if (!why?.code) return "";
  if (!WHYS.has(why.code)) return tr("verify.why.other");
  const key = `verify.why.${why.code}`;
  return why.args && Object.keys(why.args).length ? tr(key, { values: why.args }) : tr(key);
}

export const lensWord = (l: string, tr: Tr) => tr(LENSES.has(l) ? `verify.lens.${l}` : "verify.lens.other");
export const severityWord = (s: string, tr: Tr) =>
  tr(SEVERITIES.has(s) ? `verify.severity.${s}` : "verify.severity.other");
export const stepWord = (kind: string, tr: Tr) =>
  tr(KINDS.has(kind) ? `verify.round.kind.${kind}` : "verify.round.kind.other");

/** Where the work is, for one that did not land (or had nothing to land):
 *  never "waiting in its own copy" for work that has none. */
function where(place: string, tr: Tr, anyway = false): string {
  switch (place) {
    case "landed":
      return tr(anyway ? "verify.tip.landedAnyway" : "verify.tip.landed");
    case "app":
    case "helper":
    case "folder":
    case "nothing":
      return tr(`verify.place.${place}`);
    default:
      return tr("verify.tip.notLanded");
  }
}

/** The words of a badge: its label and the one sentence under it, never
 *  more than the record holds. `tr` is the caller's translator. */
export function badgeWords(
  v: VerificationView,
  tr: Tr,
): { label: string; detail: string } {
  const b = v.badge;
  const reason = b.reason as Why | null | undefined;
  const lenses = b.lenses.map((l) => lensWord(l, tr)).join(", ");
  switch (b.state) {
    case "verified": {
      const parts = [
        tr("verify.tip.verified", {
          values: { reviewers: b.reviewers, lenses, rounds: b.rounds },
        }),
      ];
      if (b.checksPassed && b.checks.length)
        parts.push(
          tr("verify.tip.checksPassed", {
            values: { checks: b.checks.join(", ") },
          }),
        );
      parts.push(tr("verify.tip.noneOpen"));
      if (b.deferred > 0)
        parts.push(
          tr("verify.tip.deferred", { values: { count: b.deferred } }),
        );
      parts.push(
        b.place !== "landed" && reason?.code
          ? whyWords(reason, tr) + "."
          : where(b.place, tr),
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
      const parts = [
        reason?.code ? whyWords(reason, tr) + "." : tr("verify.tip.notVerified"),
      ];
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
      parts.push(where(b.place, tr, true));
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
