/**
 * nebo.janus — LLM completions through Nebo's Janus gateway.
 */

import { getAppId, getBaseUrl } from './config';
import type { JanusOptions, StreamChunk } from './types';

function janusUrl(action: string): string {
  const appId = getAppId();
  const base = getBaseUrl();
  return `${base}/api/v1/apps/${appId}/janus/${action}`;
}

export const janus = {
  async complete(options: JanusOptions): Promise<string> {
    const resp = await fetch(janusUrl('complete'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(options)
    });
    const data = await resp.json();
    return data.text || data.content || '';
  },

  async *stream(options: JanusOptions): AsyncGenerator<string> {
    const resp = await fetch(janusUrl('stream'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(options)
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
            const parsed = JSON.parse(data) as StreamChunk;
            yield parsed.text;
          } catch {
            yield data;
          }
        }
      }
    }
  }
};
