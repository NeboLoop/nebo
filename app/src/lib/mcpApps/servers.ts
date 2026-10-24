// Whether a view's server is still connected, and its name: from the
// connectors list (`listIntegrations`), read at most every 30 seconds for
// every view on the page.

import { listIntegrations } from '$lib/api/nebo';

const FRESH_MS = 30_000;
let read: { at: number; list: Promise<{ id: string; name: string; connectionStatus?: string }[]> } | null = null;

export async function serverOf(id: string): Promise<{ name: string; connected: boolean } | null> {
  if (!read || Date.now() - read.at > FRESH_MS) {
    read = {
      at: Date.now(),
      list: listIntegrations()
        .then((r) => r.integrations ?? [])
        .catch(() => [])
    };
  }
  const found = (await read.list).find((i) => i.id === id);
  return found ? { name: found.name, connected: found.connectionStatus === 'connected' } : null;
}
