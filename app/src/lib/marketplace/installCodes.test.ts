/**
 * The composer reads a message of install codes the way the server does
 * (`detect_codes`, crates/server/src/codes.rs): the owner pasted
 * "CONN-V1PR-K421)" and it went to the model as chat (2026-09-30), and a
 * list of codes installs one by one.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const openCode = vi.fn();
const send = vi.fn();
vi.mock('$lib/stores/installFlow', () => ({ installFlow: { openCode: (o: unknown) => openCode(o) } }));
vi.mock('$lib/websocket/client', () => ({
  getWebSocketClient: () => ({ isConnected: () => true, send: (t: string, d: unknown) => send(t, d) }),
}));

import { matchInstallCodes, sendInstallCode } from './installCodes';

beforeEach(() => {
  openCode.mockClear();
  send.mockClear();
});

describe('matchInstallCodes', () => {
  it('ignores the punctuation a copy picks up around a code', () => {
    for (const input of ['CONN-V1PR-K421)', '(CONN-V1PR-K421)', '`CONN-V1PR-K421`', '"CONN-V1PR-K421".', '“conn-v1pr-k421”', '- CONN-V1PR-K421', '1. CONN-V1PR-K421', '1.CONN-V1PR-K421']) {
      expect(matchInstallCodes(input), input).toEqual([{ code: 'CONN-V1PR-K421', codeType: 'connection' }]);
    }
  });

  it('reads a list as every code in order, each once', () => {
    const codes = matchInstallCodes('1. SKIL-AAAA-BBBB)\n2) `plug-cccc-dddd`,\n• CONN-EEEE-FFFF; SKIL-AAAA-BBBB');
    expect(codes?.map((c) => c.code)).toEqual(['SKIL-AAAA-BBBB', 'PLUG-CCCC-DDDD', 'CONN-EEEE-FFFF']);
  });

  it('keeps a message with any other word chat', () => {
    for (const input of ['install CONN-V1PR-K421', 'CONN-V1PR-K421 please', 'CONN-V1PR-K421 2', 'NEBO-IIIL-OOOU', '', '1.']) {
      expect(matchInstallCodes(input), input).toBeNull();
    }
  });
});

describe('sendInstallCode', () => {
  it('opens the install modal for one code and delivers it', () => {
    expect(sendInstallCode('CONN-V1PR-K421)', 'assistant', 's1')).toBe(true);
    expect(openCode).toHaveBeenCalledWith(expect.objectContaining({ code: 'CONN-V1PR-K421', codeType: 'connection' }));
    expect(send).toHaveBeenCalledWith('chat', { prompt: 'CONN-V1PR-K421)', agent_id: 'assistant', session_id: 's1' });
  });

  it('delivers a list without opening the modal: the reply is the report', () => {
    expect(sendInstallCode('SKIL-AAAA-BBBB\nPLUG-CCCC-DDDD', 'assistant', 's1')).toBe(true);
    expect(openCode).not.toHaveBeenCalled();
    expect(send).toHaveBeenCalledWith('chat', { prompt: 'SKIL-AAAA-BBBB\nPLUG-CCCC-DDDD', agent_id: 'assistant', session_id: 's1' });
  });

  it('leaves chat alone', () => {
    expect(sendInstallCode('install CONN-V1PR-K421', 'assistant')).toBe(false);
    expect(send).not.toHaveBeenCalled();
  });
});
