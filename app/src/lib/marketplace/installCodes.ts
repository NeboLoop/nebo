/**
 * Canonical marketplace install-code handling.
 *
 * One regex, one type map, one instant-feedback dispatcher — shared by every
 * entry point (chat composer, chat controller, marketplace code input). Keeping
 * a single source prevents the bug where a stale copy silently dropped whole
 * code families (e.g. COLL- collections / CONN- connectors) so the install
 * modal never opened.
 */

import { get } from 'svelte/store';
import { t } from 'svelte-i18n';
import { installFlow } from '$lib/stores/installFlow';
import { getWebSocketClient } from '$lib/websocket/client';

/** PREFIX-XXXX-XXXX (Crockford Base32: no I, L, O or U). Covers every install-code family. */
export const CODE_RE =
  /^(NEBO|SKIL|WORK|AGNT|LOOP|PLUG|APPS|COLL|CONN)-[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$/i;

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

/** The install modal's first status line per code type (installFlow.codeStatus.*). */
const STATUS_BY_TYPE = new Set([
  'nebo',
  'skill',
  'workflow',
  'agent',
  'loop',
  'plugin',
  'app',
  'collection',
  'connection',
]);

/** One code, normalized, and its resolved type. */
export interface InstallCode {
  code: string;
  codeType: string;
}

/** Punctuation (anything but letters and digits) at either end of a token. */
const EDGE_PUNCTUATION = /^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu;

/** `token` as a code, or null. */
function codeAt(token: string): InstallCode | null {
  const code = token.toUpperCase();
  const m = code.match(CODE_RE);
  return m ? { code, codeType: TYPE_BY_PREFIX[m[1]] || 'code' } : null;
}

/**
 * The install codes a message holds, when it holds nothing else — the same
 * reading as the server's `detect_codes` (crates/server/src/codes.rs), which
 * installs them: one code or a list (spaces, commas, semicolons, new lines,
 * bullets, a numbered list), the punctuation a copy picks up around each
 * ignored ("CONN-V1PR-K421)"), each code once, in the order given. Null when
 * the message has any other word in it: that is chat.
 */
export function matchInstallCodes(text: string): InstallCode[] | null {
  const codes: InstallCode[] = [];
  for (const token of text.split(/[\s,;]+/)) {
    const core = token.replace(EDGE_PUNCTUATION, '');
    if (!core) continue; // a bullet, a dash, a stray bracket
    if (/^\d{1,3}$/.test(core) && core.length < token.length) continue; // "1." "2)" "(3)"
    // A numbered list's marker written against its code: "1.CONN-…".
    const found = codeAt(core) ?? codeAt(core.replace(/^\d{1,3}[.)]/, ''));
    if (!found) return null;
    if (!codes.some((c) => c.code === found.code)) codes.push(found);
  }
  return codes.length ? codes : null;
}

/**
 * Open the install modal immediately via the installFlow store — closing the gap
 * between submit and the backend's `code_processing` WS frame (which drives the
 * rest of the flow once it arrives). Returns true if `text` was an install code
 * (modal opened).
 */
export function dispatchInstallStart(text: string): boolean {
  const codes = matchInstallCodes(text);
  if (codes?.length !== 1) return false;
  openInstallModal(codes[0]);
  return true;
}

function openInstallModal(match: InstallCode) {
  installFlow.openCode({
    code: match.code,
    codeType: match.codeType,
    statusMessage: get(t)(
      STATUS_BY_TYPE.has(match.codeType) ? `installFlow.codeStatus.${match.codeType}` : 'installFlow.processing'
    ),
  });
}

/**
 * The ONE way to submit install codes: deliver the message to the backend —
 * over the WebSocket when connected, over HTTP (chatWithAgent) when not,
 * never a silent drop — which installs each code in turn and answers in the
 * conversation with a line per code. One code also opens the install modal
 * instantly, for its setup. A list opens none: each code's setup would
 * replace the last one's, so the reply's lines are the report. The chat
 * "working" spinner is not engaged here; the reply's stream engages it.
 *
 * Returns false (and does nothing) when `text` isn't only install codes.
 */
export function sendInstallCode(text: string, agentId: string, sessionId?: string): boolean {
  const codes = matchInstallCodes(text);
  if (!codes) return false;
  if (codes.length === 1) openInstallModal(codes[0]);
  const prompt = text.trim();
  void (async () => {
    try {
      const ws = getWebSocketClient();
      if (ws.isConnected()) {
        ws.send('chat', {
          prompt,
          agent_id: agentId,
          ...(sessionId ? { session_id: sessionId } : {}),
        });
      } else {
        const api = await import('$lib/api/nebo');
        await api.chatWithAgent(agentId, { prompt });
      }
    } catch (e) {
      console.warn('[nebo] Failed to submit install code', e);
    }
  })();
  return true;
}
