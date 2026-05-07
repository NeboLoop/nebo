/**
 * nebo.storage — mirrors localStorage API with server-persisted async KV.
 */

import { getAppId, getBaseUrl } from './config';

function storageUrl(key?: string): string {
  const appId = getAppId();
  const base = getBaseUrl();
  if (key) {
    return `${base}/api/v1/apps/${appId}/storage/${encodeURIComponent(key)}`;
  }
  return `${base}/api/v1/apps/${appId}/storage`;
}

export const storage = {
  async getItem(key: string): Promise<unknown | null> {
    const resp = await fetch(storageUrl(key));
    if (resp.status === 404) return null;
    const data = await resp.json();
    try {
      return JSON.parse(data.value);
    } catch {
      return data.value;
    }
  },

  async setItem(key: string, value: unknown): Promise<void> {
    const serialized = typeof value === 'string' ? value : JSON.stringify(value);
    await fetch(storageUrl(key), {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ value: serialized })
    });
  },

  async removeItem(key: string): Promise<void> {
    await fetch(storageUrl(key), { method: 'DELETE' });
  },

  async clear(): Promise<void> {
    const items = await this.keys();
    await Promise.all(items.map((key) => this.removeItem(key)));
  },

  async keys(): Promise<string[]> {
    const resp = await fetch(storageUrl());
    const data = await resp.json();
    return (data.items || []).map((item: [string, string]) => item[0]);
  }
};
