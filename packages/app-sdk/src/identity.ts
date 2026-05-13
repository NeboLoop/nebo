/**
 * nebo.identity — expose agent context (name, persona, skills, inputs) to apps.
 */

import { getAppId, getBaseUrl } from './config';

export interface AgentIdentity {
  id: string;
  name: string;
  displayName: string;
  description: string;
  persona: string;
  model: string;
  skills: string[];
  inputValues: Record<string, unknown>;
}

let _cache: AgentIdentity | null = null;

export const identity = {
  async get(): Promise<AgentIdentity> {
    if (_cache) return _cache;
    const appId = getAppId();
    const base = getBaseUrl();
    const resp = await fetch(`${base}/api/v1/apps/${appId}/identity`);
    if (!resp.ok) {
      throw new Error(`[nebo-sdk] identity fetch failed: ${resp.status}`);
    }
    _cache = await resp.json();
    return _cache!;
  },

  invalidate(): void {
    _cache = null;
  },
};
