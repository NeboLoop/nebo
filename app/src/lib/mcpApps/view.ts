// MCP Apps (io.modelcontextprotocol/ui, spec 2026-01-26) on the desktop:
// a tool result shown as the server's own interactive view, inline in the
// tool's message. The engine stores the view with the result
// (`payload.kind === 'mcp_app'`); this module is the host's side of the
// view: the sandbox proxy's address, the browser features it may use, and
// the JSON-RPC bridge over postMessage.
//
// The view's own requests are answered here (initialize, sizes, display
// mode, links). The three that need the engine (tools/call, ui/message,
// ui/update-model-context) go over the client socket as one frame,
// `mcp_app_request`, answered by `mcp_app_response` (`engine`).

/** The spec version this host speaks to views. */
export const PROTOCOL_VERSION = '2026-01-26';

export interface ViewCsp {
  connectDomains?: string[];
  resourceDomains?: string[];
  frameDomains?: string[];
  baseUriDomains?: string[];
}

export interface ViewPermissions {
  camera?: object;
  microphone?: object;
  geolocation?: object;
  clipboardWrite?: object;
}

/** A tool result's view, as the engine stores it (`payload`). */
export interface McpAppPayload {
  kind: 'mcp_app';
  serverId: string;
  tool: string;
  resourceUri: string;
  html: string;
  csp: ViewCsp;
  /** The resource declared a csp at all; without one the spec's
   *  restrictive default applies (no connections). */
  cspDeclared?: boolean;
  permissions: ViewPermissions;
  domain?: string;
  prefersBorder?: boolean;
  toolInput: Record<string, unknown>;
  toolResult: Record<string, unknown>;
}

export function isMcpApp(p: unknown): p is McpAppPayload {
  const v = p as McpAppPayload | null;
  return !!v && v.kind === 'mcp_app' && typeof v.html === 'string' && typeof v.serverId === 'string';
}

/** The proxy page's address on the view origin: the view's declared
 *  domains go along, and the engine serves the page under the policy it
 *  builds from them (the one CSP builder). `host` is this page's origin,
 *  the only one the proxy talks to. */
export function sandboxUrl(viewOrigin: string, view: Pick<McpAppPayload, 'csp' | 'cspDeclared'>, host: string): string {
  const q = new URLSearchParams({ host });
  if (view.cspDeclared !== false) q.set('csp', JSON.stringify(view.csp ?? {}));
  return `${viewOrigin.replace(/\/$/, '')}/sandbox?${q.toString()}`;
}

/** The features a view asks for that need the owner's OK (camera,
 *  microphone, location). Clipboard writing needs none. */
export function askedFeatures(perms: ViewPermissions | undefined): ('camera' | 'microphone' | 'geolocation')[] {
  const out: ('camera' | 'microphone' | 'geolocation')[] = [];
  if (perms?.camera) out.push('camera');
  if (perms?.microphone) out.push('microphone');
  if (perms?.geolocation) out.push('geolocation');
  return out;
}

/** The permissions the proxy sets on the view's frame: what the view asked
 *  for, the camera, microphone and location only once the owner allowed
 *  them. */
export function grantedPermissions(perms: ViewPermissions | undefined, allowed: boolean): ViewPermissions {
  const out: ViewPermissions = {};
  if (perms?.clipboardWrite) out.clipboardWrite = {};
  if (allowed) for (const f of askedFeatures(perms)) out[f] = {};
  return out;
}

/** The frame's `allow` attribute for the permissions granted (the
 *  spec's Permission Policy mapping). */
export function allowAttribute(perms: ViewPermissions): string {
  const out: string[] = [];
  if (perms.camera) out.push('camera');
  if (perms.microphone) out.push('microphone');
  if (perms.geolocation) out.push('geolocation');
  if (perms.clipboardWrite) out.push('clipboard-write');
  return out.join('; ');
}

export interface HostContext {
  toolInfo?: { tool: { name: string } };
  theme: 'light' | 'dark';
  locale: string;
  timeZone?: string;
  platform: 'desktop' | 'web' | 'mobile';
  displayMode: 'inline';
  availableDisplayModes: ['inline'];
  containerDimensions?: { width: number; maxHeight: number };
}

type Json = Record<string, unknown>;
interface RpcMessage {
  jsonrpc?: string;
  id?: string | number;
  method?: string;
  params?: Json;
  result?: unknown;
  error?: unknown;
}

/** What the host does for the view. */
export interface BridgeHost {
  /** Post a message to the view (through the proxy). */
  post: (msg: Json) => void;
  /** The engine's answer to tools/call, ui/message or
   *  ui/update-model-context: its result, or throws {code, message}. */
  engine: (method: string, params: Json) => Promise<unknown>;
  openLink: (url: string) => void;
  resize: (size: { width?: number; height?: number }) => void;
  context: () => HostContext;
  hostVersion: string;
}

/** The engine-bound methods. */
export const ENGINE_METHODS = new Set(['tools/call', 'ui/message', 'ui/update-model-context']);

/**
 * The host's end of one view's JSON-RPC connection. `receive` takes every
 * message from the view (after the proxy is ready); the host sends nothing
 * to the view before it says `ui/notifications/initialized`, then the
 * call's input and result.
 */
export class ViewBridge {
  private initialized = false;
  private nextId = 1;
  private pending = new Map<number, (r: RpcMessage) => void>();

  constructor(
    private host: BridgeHost,
    private call: { toolInput: Json; toolResult: Json }
  ) {}

  get ready(): boolean {
    return this.initialized;
  }

  async receive(msg: RpcMessage): Promise<void> {
    if (!msg || typeof msg !== 'object' || msg.jsonrpc !== '2.0') return;
    // A response to one of the host's requests (resource-teardown).
    if (msg.method === undefined && msg.id !== undefined) {
      const done = this.pending.get(Number(msg.id));
      if (done) {
        this.pending.delete(Number(msg.id));
        done(msg);
      }
      return;
    }
    const method = msg.method ?? '';
    if (msg.id === undefined) return this.notification(method, msg.params ?? {});
    try {
      const result = await this.request(method, msg.params ?? {});
      this.host.post({ jsonrpc: '2.0', id: msg.id, result });
    } catch (e) {
      const err = e as { code?: number; message?: string };
      this.host.post({
        jsonrpc: '2.0',
        id: msg.id,
        error: { code: typeof err?.code === 'number' ? err.code : -32000, message: err?.message ?? String(e) }
      });
    }
  }

  private notification(method: string, params: Json) {
    switch (method) {
      case 'ui/notifications/initialized':
        if (this.initialized) return;
        this.initialized = true;
        this.notify('ui/notifications/tool-input', { arguments: this.call.toolInput ?? {} });
        this.notify('ui/notifications/tool-result', this.call.toolResult ?? {});
        return;
      case 'ui/notifications/size-changed':
        this.host.resize({
          width: typeof params.width === 'number' ? params.width : undefined,
          height: typeof params.height === 'number' ? params.height : undefined
        });
        return;
      case 'notifications/message':
        console.info('[mcp-app]', params);
        return;
    }
  }

  private async request(method: string, params: Json): Promise<unknown> {
    switch (method) {
      case 'ui/initialize':
        return {
          protocolVersion: PROTOCOL_VERSION,
          hostInfo: { name: 'Nebo', version: this.host.hostVersion },
          hostCapabilities: { openLinks: {}, serverTools: {}, logging: {} },
          hostContext: this.host.context()
        };
      case 'ping':
        return {};
      case 'ui/request-display-mode':
        // Nebo shows views inline only.
        return { mode: 'inline' };
      case 'ui/open-link': {
        const url = typeof params.url === 'string' ? params.url : '';
        if (!/^https?:\/\//i.test(url)) throw { code: -32602, message: 'Invalid URL' };
        this.host.openLink(url);
        return {};
      }
    }
    if (ENGINE_METHODS.has(method)) return this.host.engine(method, params);
    throw { code: -32601, message: 'Method not found' };
  }

  notify(method: string, params: Json) {
    if (!this.initialized) return;
    this.host.post({ jsonrpc: '2.0', method, params });
  }

  /** The host context changed (theme, locale): tell the view. */
  contextChanged(partial: Partial<HostContext>) {
    this.notify('ui/notifications/host-context-changed', partial as Json);
  }

  /** The instantiating call was cancelled. Nebo shows a view once its call
   *  has finished, so this is for completeness of the bridge. */
  cancelled(reason: string) {
    this.notify('ui/notifications/tool-cancelled', { reason });
  }

  /** Before the view is removed: ask it to finish, waiting at most `ms`. */
  teardown(reason: string, ms = 500): Promise<void> {
    if (!this.initialized) return Promise.resolve();
    const id = this.nextId++;
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolve();
      }, ms);
      this.pending.set(id, () => {
        clearTimeout(timer);
        resolve();
      });
      this.host.post({ jsonrpc: '2.0', id, method: 'ui/resource-teardown', params: { reason } });
    });
  }
}
