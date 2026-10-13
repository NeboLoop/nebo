import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import type { BackgroundTask } from '$lib/api/neboComponents';
import {
  backgroundFinished,
  backgroundNotices,
  backgroundEnded,
  belongsTo,
  clock,
  launchedIn,
  kindLabel,
  rollUp,
  waitsOnOwner,
  shortClock,
  sortForPanel,
  timerCreated,
} from './background';

function task(over: Partial<BackgroundTask>): BackgroundTask {
  return {
    id: 'helper:h-1',
    kind: 'agent',
    source: 'helper',
    agentId: 'ava',
    employee: 'Ava',
    title: 'price the order',
    detail: '',
    call: null,
    status: 'running',
    wait: null,
    trigger: null,
    startedAt: 100,
    lastActivityAt: null,
    nextRunAt: null,
    createdBy: 'chat',
    sourceRunId: null,
    sessionKey: 'agent:ava:web',
    fromAgentId: null,
    fromSessionKey: null,
    launchedFrom: null,
    transcript: null,
    turnsUsed: null,
    turnsCap: null,
    actions: ['stop'],
    ...over,
  };
}

describe('background work', () => {
  beforeEach(() => {
    backgroundFinished.set([]);
    backgroundNotices.set([]);
  });

  it('belongs to its employee and to the one that passed it on; main is the empty employee', () => {
    expect(belongsTo(task({}), 'ava')).toBe(true);
    expect(belongsTo(task({}), 'ben')).toBe(false);
    expect(belongsTo(task({ agentId: 'ben', fromAgentId: 'ava' }), 'ava')).toBe(true);
    expect(belongsTo(task({ agentId: '' }), 'main')).toBe(true);
  });

  it("a chat lists what its own turns launched, and what that launched; never what fires on its own", () => {
    const chat = 'agent:main:web';
    const helper = task({ id: 'helper:h', launchedFrom: chat, transcript: { sessionKey: 'subagent:agent:main:web:h', chatId: 'c-h' } });
    const nested = task({ id: 'helper:n', launchedFrom: 'subagent:agent:main:web:h' });
    const handoff = task({ id: 'handoff:a', source: 'handoff', launchedFrom: chat, transcript: { sessionKey: 'agent:ann:coworker:main', chatId: 'c-a' } });
    const coworkerHelper = task({ id: 'helper:c', agentId: 'ann', launchedFrom: 'agent:ann:coworker:main' });
    const otherChat = task({ id: 'helper:o', launchedFrom: 'agent:main:thread:other' });
    const scheduled = task({ id: 'turn:cron', source: 'turn', createdBy: 'schedule', launchedFrom: null, sessionKey: chat });
    const triggered = task({ id: 'workflow:r', source: 'workflow', kind: 'workflow', createdBy: 'watch', launchedFrom: null, sessionKey: chat });
    const timer = task({ id: 'timer:1', source: 'timer', kind: 'loop', status: 'scheduled', createdBy: 'chat', launchedFrom: null, sessionKey: chat });
    const ids = launchedIn([nested, scheduled, helper, triggered, coworkerHelper, handoff, otherChat, timer], chat);
    expect([...ids].sort()).toEqual(['handoff:a', 'helper:c', 'helper:h', 'helper:n']);
  });

  it('lists what fires next first, soonest first, then running work oldest first', () => {
    const sorted = sortForPanel([
      task({ id: 'helper:new', startedAt: 300 }),
      task({ id: 'timer:late', source: 'timer', kind: 'loop', status: 'scheduled', nextRunAt: 900 }),
      task({ id: 'helper:old', startedAt: 100 }),
      task({ id: 'timer:soon', source: 'timer', kind: 'loop', status: 'scheduled', nextRunAt: 200 }),
    ]);
    expect(sorted.map((t) => t.id)).toEqual(['timer:soon', 'timer:late', 'helper:old', 'helper:new']);
  });

  it('reads seconds as a clock', () => {
    expect(clock(5)).toBe('5s');
    expect(clock(192)).toBe('3m 12s');
    expect(clock(3903)).toBe('1h 5m 3s');
    expect(shortClock(45)).toBe('45s');
    expect(shortClock(192)).toBe('3m 12s');
    expect(shortClock(3903)).toBe('1h 5m');
    expect(clock(-4)).toBe('0s');
  });

  it('an ended piece joins the finished list once and is told; a new timer is told', () => {
    const t = task({});
    backgroundEnded({ task: t, outcome: 'failed', endedAt: 50 });
    backgroundEnded({ task: t, outcome: 'failed', endedAt: 50 });
    expect(get(backgroundFinished)).toHaveLength(1);
    timerCreated({ task: task({ id: 'timer:7', source: 'timer', kind: 'loop' }) });
    const notices = get(backgroundNotices);
    expect(notices.map((n) => n.kind)).toEqual(['failed', 'timer']);
    expect(notices[0].at).toBe(50_000);
  });
});

describe('a row in a word', () => {
  const t = (key: string) => key;
  it('names the employee for a hand-off or a turn, else the kind of work', () => {
    expect(kindLabel(task({ source: 'handoff', employee: 'Ben' }), t)).toBe('Ben');
    expect(kindLabel(task({ source: 'turn', employee: 'Nebo' }), t)).toBe('Nebo');
    expect(kindLabel(task({ source: 'helper' }), t)).toBe('background.label.helper');
    expect(kindLabel(task({ source: 'shell', kind: 'shell' }), t)).toBe('background.label.shell');
  });
  it('waits on the owner for an approval or an answer, not for a reply', () => {
    expect(waitsOnOwner(task({ status: 'waiting', wait: 'answer' }))).toBe(true);
    expect(waitsOnOwner(task({ status: 'waiting', wait: 'approval' }))).toBe(true);
    expect(waitsOnOwner(task({ status: 'waiting', wait: 'reply' }))).toBe(false);
    expect(waitsOnOwner(task({ status: 'running' }))).toBe(false);
  });
});

describe('rolled up', () => {
  it('puts a helper a helper started under its parent, a level deeper', () => {
    const parent = task({ id: 'helper:p', transcript: { sessionKey: 'subagent:agent:ava:web:p', chatId: 'c-p' } });
    const other = task({ id: 'shell:s', source: 'shell', kind: 'shell' });
    const child = task({ id: 'helper:c', launchedFrom: 'subagent:agent:ava:web:p', transcript: { sessionKey: 'subagent:subagent:agent:ava:web:p:c', chatId: 'c-c' } });
    const grandchild = task({ id: 'helper:g', launchedFrom: 'subagent:subagent:agent:ava:web:p:c' });
    expect(rollUp([parent, other, grandchild, child]).map((r) => [r.task.id, r.depth])).toEqual([
      ['helper:p', 0],
      ['helper:c', 1],
      ['helper:g', 2],
      ['shell:s', 0],
    ]);
  });
});
