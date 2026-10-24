// A view's requests that need the engine (tools/call, ui/message,
// ui/update-model-context): one `mcp_app_request` frame on the client
// socket, answered to every client as `mcp_app_response` by its requestId.
// The engine finds the view's server from the stored result
// (chatId + messageId, the call id), never from what the view says.

import { getWebSocketClient } from '$lib/websocket/client';

export interface EngineTarget {
  chatId: string;
  /** The call id the view's result is stored under. */
  messageId: string;
}

export function engineRequest(target: EngineTarget, method: string, params: Record<string, unknown>): Promise<unknown> {
  const ws = getWebSocketClient();
  const requestId = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    const off = ws.on('mcp_app_response', (data: { requestId?: string; result?: unknown; error?: { code: number; message: string } }) => {
      if (data?.requestId !== requestId) return;
      off();
      if (data.error) reject(data.error);
      else resolve(data.result ?? {});
    });
    ws.send('mcp_app_request', { requestId, chatId: target.chatId, messageId: target.messageId, method, params });
  });
}
