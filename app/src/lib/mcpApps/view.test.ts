import { describe, expect, it, vi } from 'vitest';
import { ViewBridge, allowAttribute, grantedPermissions, isMcpApp, sandboxUrl, type BridgeHost, type HostContext } from './view';

const ctx: HostContext = {
  theme: 'dark',
  locale: 'en',
  platform: 'desktop',
  displayMode: 'inline',
  availableDisplayModes: ['inline']
};

function host(over: Partial<BridgeHost> = {}) {
  const posted: Record<string, unknown>[] = [];
  const h: BridgeHost = {
    post: (m) => posted.push(m),
    engine: vi.fn(async () => ({ content: [{ type: 'text', text: 'Booked.' }] })),
    openLink: vi.fn(),
    resize: vi.fn(),
    context: () => ctx,
    hostVersion: '1',
    ...over
  };
  return { h, posted };
}

const call = { toolInput: { place: 'Paris' }, toolResult: { content: [{ type: 'text', text: 'Paris' }] } };

describe('the view policy', () => {
  it('sends the declared domains to the view origin, which builds the policy', () => {
    const url = new URL(sandboxUrl('http://127.0.0.1:5000/', { csp: { connectDomains: ['https://a.example.com'] } }, 'http://localhost:27895'));
    expect(url.origin).toBe('http://127.0.0.1:5000');
    expect(url.pathname).toBe('/sandbox');
    expect(url.searchParams.get('host')).toBe('http://localhost:27895');
    expect(JSON.parse(url.searchParams.get('csp')!)).toEqual({ connectDomains: ['https://a.example.com'] });
  });

  it('sends no csp for a view that declared none, so the restrictive default applies', () => {
    const url = new URL(sandboxUrl('http://127.0.0.1:5000', { csp: {}, cspDeclared: false }, 'http://localhost:27895'));
    expect(url.searchParams.has('csp')).toBe(false);
  });

  it('gives camera, microphone and location only once the owner allowed them', () => {
    const asked = { camera: {}, geolocation: {}, clipboardWrite: {} };
    expect(allowAttribute(grantedPermissions(asked, false))).toBe('clipboard-write');
    expect(allowAttribute(grantedPermissions(asked, true))).toBe('camera; geolocation; clipboard-write');
    expect(allowAttribute(grantedPermissions(undefined, true))).toBe('');
  });

  it('knows a stored view', () => {
    expect(isMcpApp({ kind: 'mcp_app', html: '<p>', serverId: 's' })).toBe(true);
    expect(isMcpApp({ kind: 'search_results' })).toBe(false);
  });
});

describe('the bridge', () => {
  it('answers initialize with the host context and sends nothing before initialized', async () => {
    const { h, posted } = host();
    const b = new ViewBridge(h, call);
    await b.receive({ jsonrpc: '2.0', id: 1, method: 'ui/initialize', params: {} });
    expect(posted).toHaveLength(1);
    const r = posted[0].result as { protocolVersion: string; hostContext: HostContext };
    expect(r.protocolVersion).toBe('2026-01-26');
    expect(r.hostContext.displayMode).toBe('inline');
    expect(r.hostContext.availableDisplayModes).toEqual(['inline']);
    b.contextChanged({ theme: 'light' });
    expect(posted).toHaveLength(1);
  });

  it('sends the call input then its result once the view is initialized', async () => {
    const { h, posted } = host();
    const b = new ViewBridge(h, call);
    await b.receive({ jsonrpc: '2.0', method: 'ui/notifications/initialized' });
    await b.receive({ jsonrpc: '2.0', method: 'ui/notifications/initialized' });
    expect(posted.map((m) => m.method)).toEqual(['ui/notifications/tool-input', 'ui/notifications/tool-result']);
    expect(posted[0].params).toEqual({ arguments: { place: 'Paris' } });
    expect(posted[1].params).toEqual(call.toolResult);
    b.contextChanged({ theme: 'light', locale: 'de' });
    expect(posted[2]).toEqual({ jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: { theme: 'light', locale: 'de' } });
  });

  it('keeps every display mode request inline', async () => {
    const { h, posted } = host();
    await new ViewBridge(h, call).receive({ jsonrpc: '2.0', id: 3, method: 'ui/request-display-mode', params: { mode: 'fullscreen' } });
    expect(posted[0]).toEqual({ jsonrpc: '2.0', id: 3, result: { mode: 'inline' } });
  });

  it('sends tools/call, ui/message and ui/update-model-context to the engine', async () => {
    const { h, posted } = host();
    const b = new ViewBridge(h, call);
    await b.receive({ jsonrpc: '2.0', id: 4, method: 'tools/call', params: { name: 'book_visit', arguments: { who: 'Dana' } } });
    await b.receive({ jsonrpc: '2.0', id: 5, method: 'ui/message', params: { role: 'user', content: { type: 'text', text: 'Hi' } } });
    await b.receive({ jsonrpc: '2.0', id: 6, method: 'ui/update-model-context', params: { content: [] } });
    expect((h.engine as ReturnType<typeof vi.fn>).mock.calls.map((c) => c[0])).toEqual(['tools/call', 'ui/message', 'ui/update-model-context']);
    expect(posted[0]).toEqual({ jsonrpc: '2.0', id: 4, result: { content: [{ type: 'text', text: 'Booked.' }] } });
  });

  it('passes the engine refusal back as a JSON-RPC error', async () => {
    const { h, posted } = host({ engine: async () => Promise.reject({ code: -32602, message: 'Unknown tool: x' }) });
    await new ViewBridge(h, call).receive({ jsonrpc: '2.0', id: 7, method: 'tools/call', params: { name: 'x' } });
    expect(posted[0]).toEqual({ jsonrpc: '2.0', id: 7, error: { code: -32602, message: 'Unknown tool: x' } });
  });

  it('opens only web links, sizes the frame, and refuses unknown methods', async () => {
    const { h, posted } = host();
    const b = new ViewBridge(h, call);
    await b.receive({ jsonrpc: '2.0', id: 8, method: 'ui/open-link', params: { url: 'https://example.com' } });
    await b.receive({ jsonrpc: '2.0', id: 9, method: 'ui/open-link', params: { url: 'javascript:alert(1)' } });
    await b.receive({ jsonrpc: '2.0', method: 'ui/notifications/size-changed', params: { width: 300, height: 420 } });
    await b.receive({ jsonrpc: '2.0', id: 10, method: 'sampling/createMessage', params: {} });
    expect(h.openLink).toHaveBeenCalledWith('https://example.com');
    expect(h.openLink).toHaveBeenCalledTimes(1);
    expect((posted[1].error as { message: string }).message).toBe('Invalid URL');
    expect(h.resize).toHaveBeenCalledWith({ width: 300, height: 420 });
    expect((posted[2].error as { code: number }).code).toBe(-32601);
  });

  it('asks the view to finish before teardown and stops waiting at the limit', async () => {
    const { h, posted } = host();
    const b = new ViewBridge(h, call);
    await b.receive({ jsonrpc: '2.0', method: 'ui/notifications/initialized' });
    const done = b.teardown('closed', 50);
    const req = posted.at(-1)!;
    expect(req.method).toBe('ui/resource-teardown');
    await b.receive({ jsonrpc: '2.0', id: req.id as number, result: {} });
    await done;
  });
});
