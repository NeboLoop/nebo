// Background work: everything running, waiting or set to run without the
// owner watching it in a conversation (helpers, background commands,
// scheduled turns, workflow runs, timers, watches). `GET /background` loads
// it once; `background_changed` carries the whole list again whenever it
// changes, `background_finished` says how a piece ended, and
// `background_timer_created` names a timer just made. Nothing polls.

import { writable } from 'svelte/store';
import type { BackgroundAction, BackgroundTask, FinishedTask } from '$lib/api/neboComponents';

/** Everything in the background now, as the server orders it. */
export const backgroundTasks = writable<BackgroundTask[]>([]);

/** What ended lately, newest first. */
export const backgroundFinished = writable<FinishedTask[]>([]);

/** What a conversation is told about its employee's background work: a
 *  piece finished, failed or stopped, or a timer was made. */
export type BackgroundNotice = {
  key: string;
  kind: 'done' | 'failed' | 'stopped' | 'removed' | 'timer';
  task: BackgroundTask;
  at: number;
};

/** The notices since the app opened, oldest first. */
export const backgroundNotices = writable<BackgroundNotice[]>([]);

/** Most notices kept: the chat shows the latest few. */
const NOTICES_KEPT = 30;
/** Most finished pieces kept, as the server keeps them. */
const FINISHED_KEPT = 50;

/** Each notice once: a repeated broadcast tells nothing new. */
function notice(n: BackgroundNotice): void {
  backgroundNotices.update((list) => (list.some((m) => m.key === n.key) ? list : [...list, n].slice(-NOTICES_KEPT)));
}

export async function loadBackground(): Promise<void> {
  const { listBackground } = await import('$lib/api/nebo');
  const res = await listBackground();
  backgroundTasks.set(res.tasks ?? []);
  backgroundFinished.set(res.finished ?? []);
}

/** The whole list, as `background_changed` carries it. */
export function setBackground(data: { tasks?: BackgroundTask[] } | null | undefined): void {
  backgroundTasks.set(data?.tasks ?? []);
}

/** A piece of work ended (`background_finished`). */
export function backgroundEnded(data: FinishedTask | null | undefined): void {
  if (!data?.task) return;
  backgroundFinished.update((list) => [data, ...list.filter((f) => f.task.id !== data.task.id)].slice(0, FINISHED_KEPT));
  notice({ key: `${data.task.id}:${data.endedAt}`, kind: data.outcome, task: data.task, at: data.endedAt * 1000 });
}

/** A timer was made (`background_timer_created`). */
export function timerCreated(data: { task?: BackgroundTask } | null | undefined): void {
  if (!data?.task) return;
  notice({ key: `${data.task.id}:made`, kind: 'timer', task: data.task, at: Date.now() });
}

/** The main bot is the empty employee, whatever a caller calls it. */
function employee(agentId: string): string {
  return agentId === 'main' ? '' : agentId;
}

/** Whether `task` belongs to employee `agentId`: its own work, or work it
 *  passed to another employee. */
export function belongsTo(task: BackgroundTask, agentId: string): boolean {
  const id = employee(agentId);
  return task.agentId === id || task.fromAgentId === id;
}

/** Work that runs now (or waits), as opposed to a timer or a watch. */
export function isRunning(task: BackgroundTask): boolean {
  return task.status === 'running' || task.status === 'waiting';
}

/** Whether `task` shows in a chat's strip: work running or waiting now,
 *  and timers an employee set from a conversation. Standing setup (a
 *  workflow's schedule, a watch, a heartbeat, a schedule the owner made)
 *  lives on the dashboard, not under every chat. */
export function inStrip(task: BackgroundTask): boolean {
  return isRunning(task) || (task.source === 'timer' && task.createdBy === 'chat');
}

/** The panel's order: what fires next first (soonest first), then running
 *  work, oldest first. */
export function sortForPanel(tasks: BackgroundTask[]): BackgroundTask[] {
  const loop = (t: BackgroundTask) => (isRunning(t) ? 1 : 0);
  return [...tasks].sort((a, b) => {
    if (loop(a) !== loop(b)) return loop(a) - loop(b);
    if (!isRunning(a)) return (a.nextRunAt ?? Infinity) - (b.nextRunAt ?? Infinity);
    return (a.startedAt ?? Infinity) - (b.startedAt ?? Infinity);
  });
}

/** Seconds as "45s", "3m 12s", "1h 5m 3s". */
export function clock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h > 0) return `${h}h ${m}m ${r}s`;
  if (m > 0) return `${m}m ${r}s`;
  return `${r}s`;
}

/** Seconds as a row's short time: "45s", "3m 12s", "1h 5m". */
export function shortClock(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  return clock(s);
}

/** Take `action` on piece `id`. The list updates itself when it lands. */
export async function actOn(id: string, action: BackgroundAction): Promise<void> {
  const { backgroundAction } = await import('$lib/api/nebo');
  await backgroundAction(encodeURIComponent(id), action);
}

/** The end of piece `id`'s output, and whether it was cut. */
export async function outputOf(id: string): Promise<{ output: string; truncated: boolean }> {
  const { backgroundOutput } = await import('$lib/api/nebo');
  return backgroundOutput(encodeURIComponent(id));
}

/** Stop every helper, turn, workflow run and background command. Returns
 *  how many stopped. */
export async function stopEverything(): Promise<number> {
  const { stopAllBackground } = await import('$lib/api/nebo');
  const res = await stopAllBackground();
  return res.stopped ?? 0;
}
