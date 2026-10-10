import { describe, it, expect, beforeEach } from 'vitest';
import { get } from 'svelte/store';
import type { BackgroundTask } from '$lib/api/neboComponents';
import {
  backgroundFinished,
  backgroundNotices,
  backgroundEnded,
  belongsTo,
  clock,
  inStrip,
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

  it("a chat's strip shows running work and timers made in a chat, not standing setup", () => {
    expect(inStrip(task({}))).toBe(true);
    expect(inStrip(task({ status: 'waiting' }))).toBe(true);
    expect(inStrip(task({ source: 'timer', kind: 'loop', status: 'scheduled', createdBy: 'chat' }))).toBe(true);
    expect(inStrip(task({ source: 'timer', kind: 'loop', status: 'scheduled', createdBy: 'workflow' }))).toBe(false);
    expect(inStrip(task({ source: 'watch', kind: 'monitor', status: 'watching', createdBy: 'workflow' }))).toBe(false);
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
