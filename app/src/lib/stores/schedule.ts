import { writable, get } from 'svelte/store';
import { AGENT_ID_MAP, AGENT_ID_REVERSE, AGENTS } from '$lib/data.js';
import { ensureAgentColor } from '$lib/tokens.js';

// ─── Types ───────────────────────────────────────────────────────────
export type EventKind = 'sched' | 'event' | 'user';
export type RunStatus = 'success' | 'failed' | 'skipped' | 'running' | 'pending';

export interface RunData {
  id: string;
  status: RunStatus;
  actualDuration: string;
  startedAt: string;
  completedAt: string;
  tokens?: { input: number; output: number };
  activities?: { id: string; status: string; duration: string; output?: string; error?: string }[];
}

export interface CalendarItem {
  id: string;
  agent: string;              // short ID ('res', 'cod', etc.)
  agentFull: string;          // full ID ('researcher', 'coder', etc.)
  kind: EventKind;
  label: string;
  days: number[];             // Mon=1..Sun=7
  hour: number;               // fractional: 9.25 = 9:15 AM
  dur: number;                // fractional hours
  end: number;                // hour + dur
  workflowId?: string;
  triggerType: string;
  recurrence?: string;
  run?: RunData;
}

// ─── Schedule String Parser ──────────────────────────────────────────
const DAY_MAP: Record<string, number[]> = {
  monday: [1], tuesday: [2], wednesday: [3], thursday: [4],
  friday: [5], saturday: [6], sunday: [7],
  daily: [1, 2, 3, 4, 5, 6, 7],
  weekdays: [1, 2, 3, 4, 5],
  weekends: [6, 7],
};

export function parseScheduleString(schedule: string): { hour: number; days: number[] } | null {
  if (!schedule) return null;
  const s = schedule.trim();

  // Parse time component: "8:00 AM" or "3:00 PM"
  const timeMatch = s.match(/(\d{1,2}):(\d{2})\s*(AM|PM)/i);
  if (timeMatch) {
    let hour = parseInt(timeMatch[1]);
    const min = parseInt(timeMatch[2]);
    const ampm = timeMatch[3].toUpperCase();
    if (ampm === 'PM' && hour !== 12) hour += 12;
    if (ampm === 'AM' && hour === 12) hour = 0;
    const fractionalHour = hour + min / 60;

    // Parse day/recurrence component
    const lower = s.toLowerCase();
    for (const [key, dayNums] of Object.entries(DAY_MAP)) {
      if (lower.includes(key)) return { hour: fractionalHour, days: dayNums };
    }

    // Default: daily if no day specified
    return { hour: fractionalHour, days: [1, 2, 3, 4, 5, 6, 7] };
  }

  // Fallback: parse raw cron expression (5/6/7-field)
  const fields = s.split(/\s+/);
  if (fields.length >= 5) {
    // 7-field: sec min hour dom month dow year
    // 6-field: sec min hour dom month dow
    // 5-field: min hour dom month dow
    const offset = fields.length >= 6 ? 1 : 0;
    const cronMin = parseInt(fields[offset]);
    const cronHour = parseInt(fields[offset + 1]);
    const cronDow = fields[offset + 4] ?? '*';
    if (!isNaN(cronHour) && !isNaN(cronMin)) {
      const fractionalHour = cronHour + cronMin / 60;
      const days = parseCronDow(cronDow);
      return { hour: fractionalHour, days };
    }
  }

  return null;
}

/** Parse cron day-of-week field to ISO weekday array (Mon=1..Sun=7). */
function parseCronDow(dow: string): number[] {
  if (dow === '*') return [1, 2, 3, 4, 5, 6, 7];
  // Cron uses 0=Sun or 7=Sun, 1=Mon..6=Sat
  const cronToIso: Record<number, number> = { 0: 7, 1: 1, 2: 2, 3: 3, 4: 4, 5: 5, 6: 6, 7: 7 };
  const result = new Set<number>();
  for (const part of dow.split(',')) {
    const range = part.split('-');
    if (range.length === 2) {
      const start = parseInt(range[0]);
      const end = parseInt(range[1]);
      if (!isNaN(start) && !isNaN(end)) {
        for (let i = start; i <= end; i++) result.add(cronToIso[i] ?? i);
      }
    } else {
      const n = parseInt(part);
      if (!isNaN(n)) result.add(cronToIso[n] ?? n);
    }
  }
  return result.size > 0 ? [...result].sort() : [1, 2, 3, 4, 5, 6, 7];
}

// ─── Duration Estimation ─────────────────────────────────────────────
function parseDurationString(dur: string): number {
  // "2m 14s" → fractional hours
  const mMatch = dur.match(/(\d+)m/);
  const sMatch = dur.match(/(\d+)s/);
  const mins = mMatch ? parseInt(mMatch[1]) : 0;
  const secs = sMatch ? parseInt(sMatch[1]) : 0;
  return (mins * 60 + secs) / 3600;
}

function estimateWorkflowDuration(_agentFull: string, _workflowId: string): number {
  // Duration estimated from API run history in loadScheduleFromAPI()
  return 0.25; // default 15 min
}

function recurrenceLabel(days: number[], interval?: string): string {
  const dayPart = days.length === 7 ? 'daily' :
    days.length === 5 && days.every((d, i) => d === i + 1) ? 'weekdays' :
    days.length === 2 && days[0] === 6 && days[1] === 7 ? 'weekends' :
    (() => { const names = ['', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']; return days.map(d => names[d]).join(', '); })();
  if (interval) return `every ${interval}, ${dayPart}`;
  return dayPart;
}

// ─── Heartbeat Interval Parser ──────────────────────────────────────
function parseIntervalToMinutes(interval: string): number {
  const hMatch = interval.match(/(\d+)h/);
  const mMatch = interval.match(/(\d+)m/);
  return (hMatch ? parseInt(hMatch[1]) * 60 : 0) + (mMatch ? parseInt(mMatch[1]) : 0);
}

function parseTimeString(time: string): number {
  const match = time.match(/(\d{1,2}):(\d{2})\s*(AM|PM)/i);
  if (!match) return 0;
  let h = parseInt(match[1]);
  const m = parseInt(match[2]);
  const ampm = match[3].toUpperCase();
  if (ampm === 'PM' && h !== 12) h += 12;
  if (ampm === 'AM' && h === 12) h = 0;
  return h + m / 60;
}

function expandHeartbeat(
  interval: string,
  window?: { start: string; end: string }
): number[] {
  const intervalMin = parseIntervalToMinutes(interval);
  if (intervalMin <= 0) return [];
  const startHour = window ? parseTimeString(window.start) : 0;
  const endHour = window ? parseTimeString(window.end) : 24;
  const hours: number[] = [];
  let h = startHour;
  while (h < endHour) {
    hours.push(h);
    h += intervalMin / 60;
  }
  return hours;
}

// ─── Build Items from API (populated by loadScheduleFromAPI) ─────────
// No longer builds from static AGENT_CONFIGS — all data comes from the API.

// Event-triggered run items are now built from API cache in loadScheduleFromAPI().

function dateStringToDays(startedAt: string): number[] {
  const lower = startedAt.toLowerCase();
  if (lower.startsWith('today')) {
    const d = new Date().getDay();
    return [d === 0 ? 7 : d];
  }
  if (lower.startsWith('yesterday')) {
    const d = new Date();
    d.setDate(d.getDate() - 1);
    const wd = d.getDay();
    return [wd === 0 ? 7 : wd];
  }
  // "Monday", "Apr 26", etc. — try day name first
  for (const [name, nums] of Object.entries(DAY_MAP)) {
    if (lower.startsWith(name)) return nums;
  }
  // Try parsing as a date like "Apr 26"
  const parsed = new Date(startedAt.replace(/,.*/, '') + ', 2026');
  if (!isNaN(parsed.getTime())) {
    const wd = parsed.getDay();
    return [wd === 0 ? 7 : wd];
  }
  return [];
}

// ─── Computed schedule data ──────────────────────────────────────────
let _scheduledItems: CalendarItem[] = [];
let _eventRunItems: CalendarItem[] = [];
let _apiRunsCache: Record<string, any[]> = {};

// ─── User-created items store ────────────────────────────────────────
export const userScheduleItems = writable<CalendarItem[]>([]);

/** Ensure an agent has a short ID mapping and is in the AGENTS display list. */
function ensureAgent(agentId: string, name?: string, role?: string): string | null {
  // Check existing mapping
  let shortId = AGENT_ID_MAP[agentId];
  if (!shortId) {
    // Generate a short ID from first 3 chars
    shortId = agentId.slice(0, 3);
    AGENT_ID_MAP[agentId] = shortId;
    AGENT_ID_REVERSE[shortId] = agentId;
  }
  // Ensure agent is in the display list
  if (!AGENTS.find(a => a.id === shortId)) {
    AGENTS.push({
      id: shortId,
      initial: (name || agentId).charAt(0).toUpperCase(),
      name: name || agentId.charAt(0).toUpperCase() + agentId.slice(1),
      role: role || '',
    });
  }
  ensureAgentColor(shortId);
  return shortId;
}

/** Load schedule data from backend API (agents + workflows + runs). */
export async function loadScheduleFromAPI(): Promise<void> {
  try {
    const api = await import('$lib/api/nebo');

    // 1. Load agents to populate sidebar
    const agentsResp = await api.listAgents().catch(() => null);
    const agentIds: string[] = [];
    if (agentsResp?.agents?.length) {
      for (const a of agentsResp.agents) {
        if (!a.isEnabled) continue;
        const shortId = ensureAgent(a.id, a.name, a.description);
        if (shortId) agentIds.push(a.id);
      }
    }

    // 2. Load per-agent runs + workflows to build calendar items
    for (const agentId of agentIds) {
      try {
        const runsResp = await api.listAgentRuns(agentId).catch(() => null);
        if (runsResp?.runs?.length) {
          for (const run of runsResp.runs) {
            const wfId = (run as any).workflowRunId || (run as any).workflowId || '';
            const key = `${agentId}:${wfId}`;
            if (!_apiRunsCache[key]) _apiRunsCache[key] = [];
            _apiRunsCache[key].push(run);
          }
        }
      } catch { /* skip agent */ }
    }

    // 3. Load per-agent workflows to build calendar items
    const apiItems: CalendarItem[] = [];
    const workflowPromises = agentIds.map(async (agentId) => {
      try {
        const resp = await api.getAgentWorkflows(agentId);
        const workflowMap = resp?.workflows;
        if (!workflowMap || typeof workflowMap !== 'object') return;
        const entries = Object.entries(workflowMap);
        if (entries.length === 0) return;
        const agentShort = AGENT_ID_MAP[agentId];
        if (!agentShort) return;

        for (const [bindingName, wfData] of entries) {
          const wf = wfData as any;
          if (wf.isActive === false) continue;
          const trigger = wf.trigger || {};
          const triggerType = trigger.type || 'manual';
          const wfId = bindingName;

          // Estimate duration from run history
          const runKey = `${agentId}:${wfId}`;
          const runs = _apiRunsCache[runKey] || [];
          let dur = 0.25; // default 15 min
          if (runs.length > 0) {
            const durations = runs.map((r: any) => {
              if (r.duration) return parseDurationString(r.duration);
              return 0;
            }).filter((d: number) => d > 0);
            if (durations.length) dur = Math.max(0.25, durations.reduce((a: number, b: number) => a + b, 0) / durations.length);
          }

          if (triggerType === 'schedule') {
            const schedule = trigger.schedule || trigger.cron || '';
            const parsed = parseScheduleString(schedule);
            if (!parsed) continue;
            apiItems.push({
              id: `wf:${agentId}:${wfId}`,
              agent: agentShort, agentFull: agentId,
              kind: 'sched', label: wf.description || wfId,
              days: parsed.days, hour: parsed.hour, dur, end: parsed.hour + dur,
              workflowId: wfId, triggerType: 'schedule',
              recurrence: recurrenceLabel(parsed.days),
            });
          } else if (triggerType === 'heartbeat') {
            const interval = trigger.interval || '15m';
            const window = trigger.window;
            const hours = expandHeartbeat(interval, window);
            const days = [1, 2, 3, 4, 5, 6, 7];
            for (let i = 0; i < hours.length; i++) {
              apiItems.push({
                id: `hb:${agentId}:${wfId}:${i}`,
                agent: agentShort, agentFull: agentId,
                kind: 'sched', label: wf.description || wfId,
                days, hour: hours[i], dur, end: hours[i] + dur,
                workflowId: wfId, triggerType: 'heartbeat',
                recurrence: recurrenceLabel(days, interval),
              });
            }
          }
        }
      } catch { /* skip agent */ }
    });
    await Promise.all(workflowPromises);

    if (apiItems.length) {
      _scheduledItems = apiItems;
    }

    // 4. Build event-triggered run items from API cache
    const eventItems: CalendarItem[] = [];
    for (const [key, runs] of Object.entries(_apiRunsCache)) {
      const [agentFull, wfId] = key.split(':');
      const agentShort = AGENT_ID_MAP[agentFull];
      if (!agentShort) continue;
      // Only include event-triggered runs not already in scheduled items
      const isScheduled = apiItems.some(i => i.workflowId === wfId && i.agentFull === agentFull);
      if (isScheduled) continue;

      for (const run of runs as any[]) {
        const dateStr = run.date || '';
        const timeMatch = dateStr.match(/(\d{1,2}):(\d{2})\s*(AM|PM)/i);
        let fractionalHour = 9; // default
        if (timeMatch) {
          let h = parseInt(timeMatch[1]);
          const m = parseInt(timeMatch[2]);
          const ampm = timeMatch[3].toUpperCase();
          if (ampm === 'PM' && h !== 12) h += 12;
          if (ampm === 'AM' && h === 12) h = 0;
          fractionalHour = h + m / 60;
        }
        const runDur = run.duration ? parseDurationString(run.duration) : 0.25;
        const days = dateStringToDays(dateStr);
        if (!days.length) continue;

        eventItems.push({
          id: `ev:${agentFull}:${wfId}:${run.id}`,
          agent: agentShort, agentFull,
          kind: 'event', label: run.name || wfId,
          days, hour: fractionalHour, dur: Math.max(0.25, runDur),
          end: fractionalHour + Math.max(0.25, runDur),
          workflowId: wfId, triggerType: 'event',
          run: {
            id: run.id, status: run.status === 'completed' ? 'success' : run.status,
            actualDuration: run.duration || '',
            startedAt: dateStr,
            completedAt: '',
            tokens: run.tokens,
            activities: run.activities,
          },
        });
      }
    }
    if (eventItems.length) {
      _eventRunItems = eventItems;
    }

    // Force reactive update — _scheduledItems and _eventRunItems are non-reactive
    // module variables, so $derived blocks won't re-evaluate without this.
    userScheduleItems.update(items => [...items]);
  } catch { /* keep empty state */ }
}

let _nextUserId = 1;
export function addUserItem(item: Omit<CalendarItem, 'id' | 'kind' | 'end'>): void {
  const newItem: CalendarItem = {
    ...item,
    id: `user:${_nextUserId++}`,
    kind: 'user',
    end: item.hour + item.dur,
  };
  userScheduleItems.update(items => [...items, newItem]);
}

export function updateUserItem(id: string, changes: Partial<CalendarItem>): void {
  userScheduleItems.update(items =>
    items.map(item => {
      if (item.id !== id) return item;
      const updated = { ...item, ...changes };
      if (changes.hour !== undefined || changes.dur !== undefined) {
        updated.end = updated.hour + updated.dur;
      }
      return updated;
    })
  );
}

export function removeUserItem(id: string): void {
  userScheduleItems.update(items => items.filter(item => item.id !== id));
}

// ─── Query Functions ─────────────────────────────────────────────────

/** All schedule items (workflow + event runs + user). Call with $userScheduleItems for reactivity. */
export function getAllItems(userItems: CalendarItem[] = []): CalendarItem[] {
  return [..._scheduledItems, ..._eventRunItems, ...userItems];
}

/** Filter items for a specific weekday + enabled agents. */
export function itemsForWeekday(
  weekday: number,
  enabled: Record<string, boolean>,
  userItems: CalendarItem[] = []
): CalendarItem[] {
  return getAllItems(userItems).filter(
    item => item.days.includes(weekday) && enabled[item.agent]
  );
}

/** Flatten items with multiple hours into individual occurrences (for calendar rendering). */
export function flattenForDate(
  weekday: number,
  enabled: Record<string, boolean>,
  userItems: CalendarItem[] = []
): Array<CalendarItem & { _id: string }> {
  return itemsForWeekday(weekday, enabled, userItems)
    .map((item, idx) => ({ ...item, _id: `${item.id}-${idx}` }))
    .sort((a, b) => a.hour - b.hour || (b.end - b.hour) - (a.end - a.hour));
}

/** Attach run data to scheduled items for a given date. */
export function attachRunData(items: CalendarItem[]): CalendarItem[] {
  return items.map(item => {
    // Event items already have run data attached
    if (item.run) return item;
    // For scheduled items, find matching run in API cache
    if (item.workflowId && item.agentFull) {
      const key = `${item.agentFull}:${item.workflowId}`;
      const runs = _apiRunsCache[key];
      if (runs && runs.length > 0) {
        // Use most recent run as the "last run" for display
        const latest = runs[0];
        return {
          ...item,
          run: {
            id: latest.id,
            status: latest.status === 'completed' ? 'success' : latest.status,
            actualDuration: latest.duration || '',
            startedAt: latest.date || '',
            completedAt: '',
            tokens: latest.tokens,
            activities: latest.activities,
          },
        };
      }
    }
    return item;
  });
}

/** Get recent runs for a workflow (for DayDetailPane). */
export function getRecentRuns(agentFull: string, workflowId: string): any[] {
  const key = `${agentFull}:${workflowId}`;
  return _apiRunsCache[key] || [];
}

/** Count runs per week for sidebar display. */
export function runsPerWeek(agentShort: string, userItems: CalendarItem[] = []): number {
  // Each item already represents a single occurrence (heartbeats are already expanded)
  // so just count days per item
  return getAllItems(userItems)
    .filter(item => item.agent === agentShort && (item.kind === 'sched' || item.kind === 'user'))
    .reduce((n, item) => n + item.days.length, 0);
}

/** Agents that have schedule items (for sidebar). */
export function getScheduleAgents(userItems: CalendarItem[] = []): string[] {
  const agentIds = new Set<string>();
  for (const item of getAllItems(userItems)) {
    agentIds.add(item.agent);
  }
  // Return in discovery order (order agents were added to AGENTS array)
  return AGENTS.filter(a => agentIds.has(a.id)).map(a => a.id);
}

/** Snap an hour to the nearest 15-minute increment. */
export function snapTo15(h: number): number {
  return Math.round(h * 4) / 4;
}
