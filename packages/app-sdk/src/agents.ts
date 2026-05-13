/**
 * nebo.agents — invoke and stream agent responses.
 */

import { getAppId, getBaseUrl } from './config';
import type { AgentResponse, InvokeOptions, StreamChunk } from './types';

function agentsUrl(action: string): string {
  const appId = getAppId();
  const base = getBaseUrl();
  return `${base}/api/v1/apps/${appId}/agents/${action}`;
}

export const agents = {
  async invoke(message: string, options?: InvokeOptions): Promise<AgentResponse> {
    const resp = await fetch(agentsUrl('invoke'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        message,
        agent: options?.agent,
        data: options?.data
      })
    });
    return resp.json();
  },

  async *stream(message: string, options?: InvokeOptions): AsyncGenerator<StreamChunk> {
    const resp = await fetch(agentsUrl('stream'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        message,
        agent: options?.agent,
        data: options?.data
      })
    });

    if (!resp.body) {
      throw new Error('No response body for streaming');
    }

    const reader = resp.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';

    while (true) {
      const { done, value } = await reader.read();
      if (done) break;

      buffer += decoder.decode(value, { stream: true });
      const lines = buffer.split('\n');
      buffer = lines.pop() || '';

      for (const line of lines) {
        if (line.startsWith('data: ')) {
          const data = line.slice(6);
          if (data === '[DONE]') return;
          try {
            yield JSON.parse(data) as StreamChunk;
          } catch {
            yield { text: data, done: false };
          }
        }
      }
    }
  }
};
