import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";

/** The developer script the bot injects into an app's entry page
 *  (crates/server/src/handlers/app_devtools.js), run against a stand-in
 *  page: what the bot's live check relies on. */
const SCRIPT = readFileSync(
  fileURLToPath(new URL("../../../crates/server/src/handlers/app_devtools.js", import.meta.url)),
  "utf8",
).replace("__NEBO_DEVTOOLS_CONFIG__", JSON.stringify({ employee: "Kart", console: false }));

type Listener = (ev: unknown) => void;

class FakeWorker {
  listeners: Record<string, Listener[]> = {};
  url: string;
  constructor(url: string) {
    this.url = url;
  }
  addEventListener(type: string, f: Listener) {
    (this.listeners[type] ??= []).push(f);
  }
  fire(type: string, ev: unknown) {
    (this.listeners[type] ?? []).forEach((f) => f(ev));
  }
}

/** A page at /apps/app-1/ui/ opened as view `live-1`; `sent` are the
 *  devlog bodies that reached the bot, `land` lets the pending ones land. */
function page() {
  const sent: { entries: { level: string; message: string; view: string }[] }[] = [];
  const pending: (() => void)[] = [];
  const windowListeners: Record<string, Listener[]> = {};
  const win: Record<string, unknown> = {
    __neboViewId: "live-1",
    __neboOnReopen: (f: () => void) => f(),
    addEventListener: (type: string, f: Listener) => (windowListeners[type] ??= []).push(f),
    fetch: (_url: string, init: { body: string }) =>
      new Promise((resolve) => {
        pending.push(() => {
          sent.push(JSON.parse(init.body));
          resolve({ status: 204 });
        });
      }),
    Worker: FakeWorker,
  };
  const ctx = {
    window: win,
    document: { readyState: "loading", addEventListener: () => {}, querySelectorAll: () => [] },
    location: { pathname: "/apps/app-1/ui/", origin: "http://127.0.0.1", protocol: "http:", host: "127.0.0.1" },
    console: { log() {}, info() {}, warn() {}, error() {}, debug() {} },
    navigator: {},
    WebSocket: class {
      close() {}
    },
    XMLHttpRequest: class {},
    setTimeout,
    clearTimeout,
    Blob,
    Promise,
    JSON,
    String,
    Date,
    Math,
    Array,
    Object,
    Node: { DOCUMENT_POSITION_PRECEDING: 2 },
    getComputedStyle: () => ({}),
  };
  runInNewContext(SCRIPT, ctx);
  const land = () => pending.splice(0).forEach((f) => f());
  return { win, ctx, sent, land, windowListeners };
}

describe("the developer script", () => {
  it("tags what it sends with its view, and its flush settles only once every batch has landed", async () => {
    const p = page();
    (p.ctx.console as { error: (m: string) => void }).error("boot failed");
    let settled = false;
    const flushed = (p.win.__neboDevtoolsFlush as () => Promise<unknown>)().then(() => (settled = true));
    await Promise.resolve();
    expect(settled).toBe(false);
    p.land();
    await flushed;
    expect(settled).toBe(true);
    expect(p.sent.flatMap((b) => b.entries)).toContainEqual(
      expect.objectContaining({ level: "error", message: "boot failed", view: "live-1" }),
    );
  });

  it("keeps a subclass of Worker working and reports a worker's uncaught error", async () => {
    const p = page();
    const W = p.win.Worker as typeof FakeWorker;
    class Mine extends W {}
    const w = new Mine("worker.js");
    expect(w).toBeInstanceOf(Mine);
    expect(w).toBeInstanceOf(FakeWorker);
    w.fire("error", { message: "kaboom", filename: "http://127.0.0.1/apps/app-1/ui/worker.js", lineno: 3, colno: 9 });
    const flushed = (p.win.__neboDevtoolsFlush as () => Promise<unknown>)();
    p.land();
    await flushed;
    expect(p.sent.flatMap((b) => b.entries).map((e) => e.message)).toContain(
      "Worker: kaboom (/apps/app-1/ui/worker.js:3:9)",
    );
  });

  it("says when it has taken its first look at the page after the load", () => {
    vi.useFakeTimers();
    try {
      const p = page();
      expect(p.win.__neboLooked).toBeUndefined();
      (p.windowListeners.load ?? []).forEach((f) => f({}));
      vi.advanceTimersByTime(2600);
      expect(p.win.__neboLooked).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});
