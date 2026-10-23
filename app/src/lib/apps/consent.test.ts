import { describe, it, expect, vi, beforeEach } from "vitest";
import { get } from "svelte/store";
import en from "$lib/i18n/locales/en.json";

const getAppPermissions = vi.fn();
vi.mock("$lib/api/nebo", () => ({
  getAppPermissions: (id: string) => getAppPermissions(id),
}));

const { askAppPermissions, consentAsk, permissionWords } =
  await import("./consent");

const files = {
  permission: "filesystem:read:Media/Projects",
  kind: "files",
  target: "NeboAI › Media › Projects",
  state: "asking",
};
const anyWeb = {
  permission: "network:*",
  kind: "web",
  target: "",
  state: "allowed",
};

describe("app consent", () => {
  beforeEach(() => {
    getAppPermissions.mockReset();
    consentAsk.set(null);
  });

  it("says each request in plain words, with an English line for every one", () => {
    const words = permissionWords(files, "Media Studio", true);
    expect(words.key).toBe("appPermissions.wantsFiles");
    const line = (en.appPermissions as Record<string, string>)[
      words.key.split(".")[1]
    ];
    expect(
      line
        .replace("{name}", "Media Studio")
        .replace("{target}", words.values.target),
    ).toBe("Media Studio wants to see the files in NeboAI › Media › Projects");
    for (const kind of ["files", "web", "helper"])
      for (const target of ["x", ""])
        for (const asking of [true, false]) {
          if (kind === "files" && target === "") continue;
          const k = permissionWords(
            { ...files, kind, target },
            "A",
            asking,
          ).key.split(".")[1];
          expect(
            (en.appPermissions as Record<string, string>)[k],
            k,
          ).toBeTruthy();
        }
    expect(permissionWords(anyWeb, "A", false).key).toBe(
      "appPermissions.canWebAny",
    );
  });

  it("opens at once when nothing waits on the owner", async () => {
    getAppPermissions.mockResolvedValue({ own: false, permissions: [anyWeb] });
    await askAppPermissions("app-1", "A");
    expect(get(consentAsk)).toBeNull();
    getAppPermissions.mockResolvedValue({ own: true, permissions: [files] });
    await askAppPermissions("app-1", "A");
    expect(get(consentAsk)).toBeNull();
    getAppPermissions.mockRejectedValue(new Error("offline"));
    await askAppPermissions("app-1", "A");
    expect(get(consentAsk)).toBeNull();
  });

  it("asks about what is not answered, and opens once he is done", async () => {
    getAppPermissions.mockResolvedValue({
      own: false,
      permissions: [files, anyWeb],
    });
    let opened = false;
    const open = askAppPermissions("app-1", "Media Studio").then(
      () => (opened = true),
    );
    await vi.waitFor(() => expect(get(consentAsk)).not.toBeNull());
    const ask = get(consentAsk)!;
    expect(ask.permissions.map((p) => p.permission)).toEqual([
      "filesystem:read:Media/Projects",
    ]);
    expect(opened).toBe(false);
    ask.done();
    await open;
    expect(opened).toBe(true);
  });
});
