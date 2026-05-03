import { N } from './tokens.js';

export function triggerGlyph(kind) {
  if (kind === 'event') return '⚡';
  if (kind === 'user')  return '›';
  return '↻';
}

export function fmtTime(h) {
  const whole = Math.floor(h);
  const mins = Math.round((h - whole) * 60);
  if (whole === 12 && mins === 0) return 'Noon';
  if (whole === 0  && mins === 0) return 'Midnight';
  const ampm   = whole < 12 ? 'AM' : 'PM';
  const display = whole % 12 === 0 ? 12 : whole % 12;
  return mins ? `${display}:${String(mins).padStart(2, '0')} ${ampm}` : `${display} ${ampm}`;
}

export function fmtHour(h) {
  if (h === 12) return 'Noon';
  if (h === 0)  return 'Midnight';
  return `${h % 12 === 0 ? 12 : h % 12} ${h < 12 ? 'AM' : 'PM'}`;
}

export function eventBlockColor(kind) {
  if (kind === 'sched') return { bg: N.active, fg: '#fff', border: 'none' };
  if (kind === 'event') return { bg: N.needsBg, fg: N.needs, border: `1px solid ${N.needs}` };
  return {
    bg: `repeating-linear-gradient(45deg, #F0EEE7 0 4px, #F7F5EE 4px 8px)`,
    fg: N.ink2,
    border: `1px dashed ${N.ink4}`,
  };
}

/** Apple-Calendar lane packing — overlap-aware column layout. */
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
