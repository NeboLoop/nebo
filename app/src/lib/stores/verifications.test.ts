import { describe, expect, it } from "vitest";
import type { VerificationView } from "$lib/api/neboComponents";
import { badgeWords, cost, inProgress, onMessage } from "./verifications";

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
      reason: "",
      landed: "abc123",
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
        reason: "No tests or build could run",
        landed: "",
        openBlockers: 1,
        openMajors: 2,
      }),
      tr,
    );
    expect(w.label).toBe("verify.badge.notVerified");
    expect(w.detail).toContain("No tests or build could run.");
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

  it("reads a cost in dollars", () => {
    expect(cost(0)).toBe("$0");
    expect(cost(50_000)).toBe("<$0.01");
    expect(cost(123_000_000)).toBe("$1.23");
  });
});
