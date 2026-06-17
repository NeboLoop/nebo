/**
 * Canonical marketplace install-code handling.
 *
 * One regex, one type map, one instant-feedback dispatcher — shared by every
 * entry point (chat composer, chat controller, marketplace code input). Keeping
 * a single source prevents the bug where a stale copy silently dropped whole
 * code families (e.g. COLL- collections / CONN- connectors) so the install
 * modal never opened.
 */

import { installFlow } from '$lib/stores/installFlow';

/** PREFIX-XXXX-XXXX (Crockford Base32). Covers every install-code family. */
export const CODE_RE = /^(NEBO|SKIL|WORK|AGNT|LOOP|PLUG|APPS|COLL|CONN)-[0-9A-Z]{4}-[0-9A-Z]{4}$/i;

const TYPE_BY_PREFIX: Record<string, string> = {
  NEBO: 'nebo',
  SKIL: 'skill',
  WORK: 'workflow',
  AGNT: 'agent',
  LOOP: 'loop',
  PLUG: 'plugin',
  APPS: 'app',
  COLL: 'collection',
  CONN: 'connection',
};

const STATUS_BY_TYPE: Record<string, string> = {
  nebo: 'Connecting to NeboAI...',
  skill: 'Installing skill...',
  workflow: 'Installing workflow...',
  agent: 'Installing agent...',
  loop: 'Joining loop...',
  plugin: 'Installing plugin...',
  app: 'Installing app...',
  collection: 'Installing collection...',
  connection: 'Adding MCP connection...',
};

/** The normalized code and its resolved type, or null if `text` isn't a code. */
export function matchInstallCode(text: string): { code: string; codeType: string } | null {
  const code = text.trim().toUpperCase();
  const m = code.match(CODE_RE);
  if (!m) return null;
  return { code, codeType: TYPE_BY_PREFIX[m[1].toUpperCase()] || 'code' };
}

/**
 * Open the install modal immediately via the installFlow store — closing the gap
 * between submit and the backend's `code_processing` WS frame (which drives the
 * rest of the flow once it arrives). Returns true if `text` was an install code
 * (modal opened).
 */
export function dispatchInstallStart(text: string): boolean {
  const match = matchInstallCode(text);
  if (!match) return false;
  installFlow.openCode({
    code: match.code,
    codeType: match.codeType,
    statusMessage: STATUS_BY_TYPE[match.codeType] || 'Processing...',
    // User-initiated from the desktop UI — the modal stays open until dismissed.
    interactive: true,
  });
  return true;
}
