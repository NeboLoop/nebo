// Demo agents — mapped to Nebo's existing --agent-* CSS variable palette.
export const AGENTS = [
  { id: 'ops', initial: 'O', name: 'Ops',        role: 'Calendar & email',     color: 'amber' },
  { id: 'res', initial: 'R', name: 'Researcher',  role: 'Web + market data',    color: 'sky' },
  { id: 'soc', initial: 'S', name: 'Social',      role: 'Posting & scheduling', color: 'lilac' },
  { id: 'mkt', initial: 'M', name: 'Marketing',   role: 'Strategy & copy',      color: 'rose' },
  { id: 'cod', initial: 'C', name: 'Coder',       role: 'TypeScript fullstack', color: 'green' },
  { id: 'tst', initial: 'T', name: 'Tester',      role: 'QA & e2e',            color: 'peach' },
];

export const SCHED_AGENTS = AGENTS.map(a => a.id);
export const CAL_DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

export const SCHEDULE = [
  { agent: 'res', kind: 'sched', label: 'Market scan',     days: [1,2,3,4,5], hours: [9,11,13,15], dur: 0.5 },
  { agent: 'res', kind: 'sched', label: 'Daily digest',    days: [1,2,3,4,5], hours: [17],          dur: 1.0 },
  { agent: 'res', kind: 'event', label: 'On news mention', days: [1,2,3,4,5], hours: [10.4,14.2],  dur: 0.4 },
  { agent: 'soc', kind: 'sched', label: 'Morning post',    days: [1,2,3,4,5], hours: [9],          dur: 0.5 },
  { agent: 'soc', kind: 'sched', label: 'Afternoon post',  days: [1,2,3,4,5], hours: [15],         dur: 0.5 },
  { agent: 'soc', kind: 'event', label: 'On @mention',     days: [2,4],       hours: [11.5,16.2],  dur: 0.4 },
  { agent: 'ops', kind: 'sched', label: 'AM calendar',     days: [1,2,3,4,5], hours: [8],          dur: 0.5 },
  { agent: 'ops', kind: 'sched', label: 'EOD wrap',        days: [1,2,3,4,5], hours: [18],         dur: 0.5 },
  { agent: 'ops', kind: 'event', label: 'On new email',    days: [1,2,3,4,5], hours: [10.7,13.3,15.6], dur: 0.3 },
  { agent: 'mkt', kind: 'sched', label: 'Weekly strategy', days: [1],         hours: [10],         dur: 2.0 },
  { agent: 'mkt', kind: 'user',  label: 'You · launch Q&A', days: [3],        hours: [14.2],       dur: 0.7 },
  { agent: 'cod', kind: 'sched', label: 'Nightly build',   days: [1,2,3,4,5], hours: [2],          dur: 1.5 },
  { agent: 'cod', kind: 'event', label: 'On PR opened',    days: [2,3,4],     hours: [10.9,13.6,16.1], dur: 0.5 },
  { agent: 'cod', kind: 'user',  label: 'You · debug',     days: [3],         hours: [11.2],       dur: 0.8 },
  { agent: 'tst', kind: 'event', label: 'On commit',       days: [2,3,4],     hours: [11.5,14.1,16.6], dur: 0.4 },
  { agent: 'tst', kind: 'sched', label: 'Nightly e2e',     days: [1,2,3,4,5], hours: [3.5],        dur: 1.0 },
];

/** Get CSS variable pair for an agent id. */
export function agentColor(agentId) {
  const a = AGENTS.find(x => x.id === agentId);
  if (!a) return { bg: 'var(--agent-slate-bg)', ink: 'var(--agent-slate-ink)' };
  return { bg: `var(--agent-${a.color}-bg)`, ink: `var(--agent-${a.color}-ink)` };
}

export function triggerGlyph(kind) {
  if (kind === 'event') return '⚡';
  if (kind === 'user')  return '›';
  return '↻';
}

export function fmtTime(h) {
  const whole = Math.floor(h);
  const mins = h % 1 ? 30 : 0;
  if (whole === 12 && mins === 0) return 'Noon';
  if (whole === 0  && mins === 0) return 'Midnight';
  const ampm   = whole < 12 ? 'AM' : 'PM';
  const display = whole % 12 === 0 ? 12 : whole % 12;
  return mins ? `${display}:30 ${ampm}` : `${display} ${ampm}`;
}

export function fmtHour(h) {
  if (h === 12) return 'Noon';
  if (h === 0)  return 'Midnight';
  return `${h % 12 === 0 ? 12 : h % 12} ${h < 12 ? 'AM' : 'PM'}`;
}

export function packLanes(items) {
  const laneOf = items.map(() => 0);
  const lanes  = [];
  items.forEach((it, i) => {
    for (let li = 0; li < lanes.length; li++) {
      if (it.hour >= lanes[li]) { lanes[li] = it.end; laneOf[i] = li; return; }
    }
    lanes.push(it.end);
    laneOf[i] = lanes.length - 1;
  });
  const totalLanes = lanes.length || 1;
  return items.map((it, idx) => ({ ...it, lane: laneOf[idx], totalLanes }));
}

export function runsPerWeek(agentId) {
  return SCHEDULE.filter(s => s.agent === agentId)
    .reduce((n, s) => n + s.hours.length * s.days.length, 0);
}
