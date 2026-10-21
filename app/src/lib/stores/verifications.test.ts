import { describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";
import type { VerificationView } from "$lib/api/neboComponents";

const socket = vi.hoisted(() => ({
  status: [] as ((s: string) => void)[],
  on: vi.fn(),
  onStatus(fn: (s: string) => void) {
    this.status.push(fn);
    return () => {};
  },
}));
const listed = vi.hoisted(() => ({ calls: [] as string[], rows: [] as unknown[] }));
vi.mock("$lib/websocket/client", () => ({ getWebSocketClient: () => socket }));
vi.mock("$lib/api/nebo", () => ({
  listVerifications: async (_chat: unknown, session: string) => {
    listed.calls.push(session);
    return { verifications: listed.rows };
  },
}));

import {
  badgeWords,
  cost,
  inProgress,
  onMessage,
  trackConversation,
  verifications,
  whyWords,
} from "./verifications";

const tr = (key: string, opts?: { values: Record<string, string | number> }) =>
  opts
    ? `${key}(${Object.entries(opts.values)
        .map(([k, v]) => `${k}=${v}`)
        .join(",")})`
    : key;

function view(
  badge: Partial<VerificationView["badge"]>,
  extra: Partial<VerificationView> = {},
): VerificationView {
  return {
    id: "v-1",
    sessionKey: "agent:dev:web",
    messageId: "m-1",
    history: [],
    costMicrocents: 0,
    badge: {
      state: "verified",
      reviewers: 3,
      lenses: ["correctness", "billing"],
      rounds: 2,
      checks: ["Rust build", "Rust tests"],
      checksPassed: true,
      openBlockers: 0,
      openMajors: 0,
      openMinors: 0,
      deferred: 0,
      reason: null,
      landed: "abc123",
      place: "landed",
      ...badge,
    },
    ...extra,
  } as VerificationView;
}

describe("the badge", () => {
  it("says verified with who checked it, what passed, and never more than that", () => {
    const w = badgeWords(view({}), tr);
    expect(w.label).toBe("verify.badge.verified");
    expect(w.detail).toContain(
      "verify.tip.verified(reviewers=3,lenses=verify.lens.correctness, verify.lens.billing,rounds=2)",
    );
    expect(w.detail).toContain(
      "verify.tip.checksPassed(checks=Rust build, Rust tests)",
    );
    expect(w.detail).toContain("verify.tip.landed");
    expect(w.detail).toContain("verify.tip.limit");
  });

  it("says why it is not verified and what is open", () => {
    const w = badgeWords(
      view({
        state: "not_verified",
        reason: { code: "no_checks" },
        landed: "",
        place: "copy",
        openBlockers: 1,
        openMajors: 2,
      }),
      tr,
    );
    expect(w.label).toBe("verify.badge.notVerified");
    expect(w.detail).toContain("verify.why.no_checks.");
    expect(w.detail).toContain("verify.tip.open(blockers=1,majors=2,rounds=2)");
    expect(w.detail).toContain("verify.tip.notLanded");
  });

  it("sits on the reply that reported it, or shows while it is checked", () => {
    const done = view({});
    const checking = view(
      { state: "checking" },
      { id: "v-2", messageId: undefined },
    );
    const all = { [done.id]: done, [checking.id]: checking };
    expect(onMessage(all, "m-1").map((v) => v.id)).toEqual(["v-1"]);
    expect(inProgress(all, "agent:dev:web").map((v) => v.id)).toEqual(["v-2"]);
  });

  it("words the bot's codes, never a raw code or sentence", () => {
    expect(
      whyWords({ code: "busy_branch", args: { now: "experiment", from: "main" } }, tr),
    ).toBe("verify.why.busy_branch(now=experiment,from=main)");
    expect(whyWords({ code: "from_a_newer_bot" }, tr)).toBe("verify.why.other");
    for (const code of ["restarted", "unreviewed", "needs_review", "changed_after_review", "waiting_to_land", "live_unavailable", "live_no_script"]) {
      expect(whyWords({ code }, tr)).toBe(`verify.why.${code}`);
    }
    expect(whyWords(null, tr)).toBe("");
  });

  it("says where the work is by what it is, never a copy it does not have", () => {
    for (const place of ["app", "helper", "folder", "nothing"]) {
      const w = badgeWords(view({ landed: "", place }), tr);
      expect(w.detail).toContain(`verify.place.${place}`);
      expect(w.detail).not.toContain("verify.tip.notLanded");
    }
    expect(badgeWords(view({ landed: "", place: "copy" }), tr).detail).toContain(
      "verify.tip.notLanded",
    );
  });

  it("names the checks only when they passed", () => {
    const w = badgeWords(view({ checksPassed: false }), tr);
    expect(w.detail).not.toContain("verify.tip.checksPassed");
    expect(badgeWords(view({ checks: [] }), tr).detail).not.toContain(
      "verify.tip.checksPassed",
    );
  });

  it("reads a conversation's again when the socket comes back", async () => {
    listed.rows = [view({ state: "checking" }, { messageId: undefined })];
    await trackConversation("agent:dev:web");
    expect(listed.calls).toEqual(["agent:dev:web"]);
    listed.rows = [view({ state: "verified" })];
    for (const fn of socket.status) fn("disconnected");
    for (const fn of socket.status) fn("connected");
    await vi.waitFor(() => expect(listed.calls.length).toBe(2));
    await vi.waitFor(() =>
      expect(get(verifications)["v-1"].badge.state).toBe("verified"),
    );
  });

  it("reads a cost in dollars", () => {
    expect(cost(0)).toBe("$0");
    expect(cost(50_000)).toBe("<$0.01");
    expect(cost(123_000_000)).toBe("$1.23");
  });
});
