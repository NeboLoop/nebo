import { describe, expect, it } from "vitest";
import { historyItems, workspacePathOf } from "./fileHistory";

describe("workspacePathOf", () => {
  it("decodes the path a file is opened by", () => {
    expect(workspacePathOf("/api/v1/files/reports/Q3%20model.xlsx")).toBe(
      "reports/Q3 model.xlsx",
    );
    expect(workspacePathOf("/api/v1/files/a.md?preview=pdf")).toBe("a.md");
  });
  it("is null for anything that is not a workspace file", () => {
    expect(workspacePathOf("https://example.com/a.md")).toBeNull();
    expect(workspacePathOf("/api/v1/files/")).toBeNull();
    expect(workspacePathOf("/api/v1/files/%E0%A4%A")).toBeNull();
  });
});

describe("historyItems", () => {
  it("keeps the well-formed entries", () => {
    const good = {
      id: 3,
      url: "/api/v1/files/work/blobs/h.md",
      sizeBytes: 5,
      reason: "modified",
      capturedAt: 1,
    };
    expect(historyItems({ entries: [good, { id: "x" }, null] })).toEqual([
      good,
    ]);
    expect(historyItems(undefined)).toEqual([]);
  });
});
