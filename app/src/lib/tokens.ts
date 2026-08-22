// Design tokens — warm neutral palette, Geist type.
export const N = {
  bg:           '#FAFAF7',
  surface:      '#FFFFFF',
  inset:        '#F4F2EC',
  hairline:     '#E8E5DD',
  hairlineSoft: '#EFECE4',

  ink:   '#1A1815',
  ink2:  '#3A362F',
  ink3:  '#6B665C',
  ink4:  '#9A958A',

  active:    '#5C7A5A',
  activeBg:  '#EEF2EC',
  needs:     '#A36B1F',
  needsBg:   '#F7EFE0',
  idle:      '#9A958A',
  idleBg:    '#F0EEE7',
  done:      '#6B665C',
  blocked:   '#A85A4A',
  blockedBg: '#F4E8E5',

  font: "'Geist', system-ui, -apple-system, sans-serif",
  mono: "'Geist Mono', ui-monospace, SFMono-Regular, monospace",
};

// Per-agent hue palette (Apple Calendar style) — uses CSS vars from app.css.
// Per-agent hue palette — agents loaded from API, colors assigned by short ID.
export const AGENT_COLORS: Record<string, { fillClass: string; dotClass: string; edgeClass: string; textClass: string; checkboxClass: string; edgeVar: string; fillVar: string }> = {
  ops: { fillClass: 'bg-[var(--agent-mint-bg)]',  dotClass: 'bg-[var(--agent-mint-ink)]',  edgeClass: 'border-[var(--agent-mint-ink)]',  textClass: 'text-[var(--agent-mint-ink)]',  checkboxClass: 'checkbox-accent',   edgeVar: 'var(--agent-mint-ink)',  fillVar: 'var(--agent-mint-bg)' },
  res: { fillClass: 'bg-[var(--agent-green-bg)]', dotClass: 'bg-[var(--agent-green-ink)]', edgeClass: 'border-[var(--agent-green-ink)]', textClass: 'text-[var(--agent-green-ink)]', checkboxClass: 'checkbox-success',  edgeVar: 'var(--agent-green-ink)', fillVar: 'var(--agent-green-bg)' },
  soc: { fillClass: 'bg-[var(--agent-rose-bg)]',  dotClass: 'bg-[var(--agent-rose-ink)]',  edgeClass: 'border-[var(--agent-rose-ink)]',  textClass: 'text-[var(--agent-rose-ink)]',  checkboxClass: 'checkbox-error',    edgeVar: 'var(--agent-rose-ink)',  fillVar: 'var(--agent-rose-bg)' },
  mkt: { fillClass: 'bg-[var(--agent-amber-bg)]', dotClass: 'bg-[var(--agent-amber-ink)]', edgeClass: 'border-[var(--agent-amber-ink)]', textClass: 'text-[var(--agent-amber-ink)]', checkboxClass: 'checkbox-warning',  edgeVar: 'var(--agent-amber-ink)', fillVar: 'var(--agent-amber-bg)' },
  cod: { fillClass: 'bg-[var(--agent-sky-bg)]',   dotClass: 'bg-[var(--agent-sky-ink)]',   edgeClass: 'border-[var(--agent-sky-ink)]',   textClass: 'text-[var(--agent-sky-ink)]',   checkboxClass: 'checkbox-info',     edgeVar: 'var(--agent-sky-ink)',   fillVar: 'var(--agent-sky-bg)' },
  tst: { fillClass: 'bg-[var(--agent-slate-bg)]', dotClass: 'bg-[var(--agent-slate-ink)]', edgeClass: 'border-[var(--agent-slate-ink)]', textClass: 'text-[var(--agent-slate-ink)]', checkboxClass: 'checkbox-neutral',  edgeVar: 'var(--agent-slate-ink)', fillVar: 'var(--agent-slate-bg)' },
};

// Color cycle for dynamically loaded agents
const CALENDAR_COLOR_ORDER = ['violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach', 'lilac'];
const CHECKBOX_CYCLE = ['checkbox-primary', 'checkbox-success', 'checkbox-info', 'checkbox-warning', 'checkbox-error', 'checkbox-accent', 'checkbox-neutral', 'checkbox-secondary', 'checkbox-primary'];
let _colorIndex = 0;

function extractVar(cls: string): string {
  const match = cls.match(/\[(var\([^)]+\))\]/);
  return match ? match[1] : '';
}

/** Ensure a short agent ID has a color entry in AGENT_COLORS. */
export function ensureAgentColor(shortId: string): void {
  if (AGENT_COLORS[shortId]) return;
  const colorName = CALENDAR_COLOR_ORDER[_colorIndex % CALENDAR_COLOR_ORDER.length];
  const cbClass = CHECKBOX_CYCLE[_colorIndex % CHECKBOX_CYCLE.length];
  _colorIndex++;
  const base = AGENT_COLORS_MAP[colorName];
  AGENT_COLORS[shortId] = {
    fillClass: base.bgClass,
    dotClass: base.inkClass.replace('text-', 'bg-'),
    edgeClass: base.borderClass,
    textClass: base.inkClass,
    checkboxClass: cbClass,
    edgeVar: extractVar(base.borderClass) || base.borderClass,
    fillVar: extractVar(base.bgClass) || base.bgClass,
  };
}

// Agent color map by color name (used for agent avatar backgrounds)
export const AGENT_COLORS_MAP: Record<string, { bgClass: string; inkClass: string; borderClass: string }> = {
  violet: { bgClass: 'bg-[var(--agent-violet-bg)]', inkClass: 'text-[var(--agent-violet-ink)]', borderClass: 'border-[var(--agent-violet-ink)]' },
  green:  { bgClass: 'bg-[var(--agent-green-bg)]',  inkClass: 'text-[var(--agent-green-ink)]',  borderClass: 'border-[var(--agent-green-ink)]' },
  sky:    { bgClass: 'bg-[var(--agent-sky-bg)]',    inkClass: 'text-[var(--agent-sky-ink)]',    borderClass: 'border-[var(--agent-sky-ink)]' },
  amber:  { bgClass: 'bg-[var(--agent-amber-bg)]',  inkClass: 'text-[var(--agent-amber-ink)]',  borderClass: 'border-[var(--agent-amber-ink)]' },
  rose:   { bgClass: 'bg-[var(--agent-rose-bg)]',   inkClass: 'text-[var(--agent-rose-ink)]',   borderClass: 'border-[var(--agent-rose-ink)]' },
  mint:   { bgClass: 'bg-[var(--agent-mint-bg)]',   inkClass: 'text-[var(--agent-mint-ink)]',   borderClass: 'border-[var(--agent-mint-ink)]' },
  slate:  { bgClass: 'bg-[var(--agent-slate-bg)]',  inkClass: 'text-[var(--agent-slate-ink)]',  borderClass: 'border-[var(--agent-slate-ink)]' },
  peach:  { bgClass: 'bg-[var(--agent-peach-bg)]',  inkClass: 'text-[var(--agent-peach-ink)]',  borderClass: 'border-[var(--agent-peach-ink)]' },
  lilac:  { bgClass: 'bg-[var(--agent-lilac-bg)]',  inkClass: 'text-[var(--agent-lilac-ink)]',  borderClass: 'border-[var(--agent-lilac-ink)]' },
  teal:   { bgClass: 'bg-primary/15',               inkClass: 'text-primary',                   borderClass: 'border-primary' },
};

/**
 * The ONE place an employee's colour is decided.
 *
 * A stored colour always wins. Employees created without one — marketplace
 * installs don't set it — used to fall back to `teal`, which is `bg-primary`,
 * so a roster of five colourless employees rendered as five identical
 * lavender avatars and threw away the only signal you can read at a glance in
 * a 48px rail. They now get a stable colour derived from their id: same
 * employee, same colour, on every surface and across reloads.
 *
 * `teal` is the brand colour and stays reserved for the primary employee.
 */
const FALLBACK_PALETTE = ['violet', 'green', 'sky', 'amber', 'rose', 'mint', 'slate', 'peach', 'lilac'];

export function agentColorName(agentId: string, stored?: string | null): string {
  if (stored && AGENT_COLORS_MAP[stored]) return stored;
  if (agentId === 'assistant') return 'teal';
  let h = 0;
  for (let i = 0; i < agentId.length; i++) h = (h * 31 + agentId.charCodeAt(i)) >>> 0;
  return FALLBACK_PALETTE[h % FALLBACK_PALETTE.length];
}

/**
 * Colours for a whole roster, coordinated so no two employees collide while
 * any hue is still free. The per-id hash alone is stable but collision-blind:
 * a roster with a stored `peach` and a second employee hashing to peach gives
 * you two identical avatars, which is exactly the confusion colour is meant to
 * remove. Stored colours claim their hue first; everyone else takes the next
 * free one from their own stable starting point.
 */
export function assignAgentColors(
  agents: Array<{ id: string; color?: string | null }>
): Record<string, string> {
  const out: Record<string, string> = {};
  const taken = new Set<string>();
  for (const a of agents) {
    if (a.color && AGENT_COLORS_MAP[a.color]) {
      out[a.id] = a.color;
      taken.add(a.color);
    } else if (a.id === 'assistant') {
      out[a.id] = 'teal';
      taken.add('teal');
    }
  }
  // Sorted so the assignment doesn't shuffle when the roster arrives in a
  // different order.
  for (const a of [...agents].sort((x, y) => x.id.localeCompare(y.id))) {
    if (out[a.id]) continue;
    let pick = agentColorName(a.id, null);
    if (taken.size < FALLBACK_PALETTE.length) {
      let i = FALLBACK_PALETTE.indexOf(pick);
      while (taken.has(pick)) {
        i = (i + 1) % FALLBACK_PALETTE.length;
        pick = FALLBACK_PALETTE[i];
      }
    }
    out[a.id] = pick;
    taken.add(pick);
  }
  return out;
}

export function agentColors(agentId: string, stored?: string | null) {
  return AGENT_COLORS_MAP[agentColorName(agentId, stored)];
}

// Advisor role colors
export const ADVISOR_ROLE_COLORS: Record<string, { barClass: string; textClass: string }> = {
  critic:          { barClass: 'bg-error',            textClass: 'text-error' },
  builder:         { barClass: 'bg-info',             textClass: 'text-info' },
  historian:       { barClass: 'bg-secondary',        textClass: 'text-secondary' },
  strategist:      { barClass: 'bg-success',          textClass: 'text-success' },
  analyst:         { barClass: 'bg-warning',          textClass: 'text-warning' },
  innovator:       { barClass: 'bg-accent',           textClass: 'text-accent' },
  'user-advocate': { barClass: 'bg-primary',          textClass: 'text-primary' },
  general:         { barClass: 'bg-base-content/50',  textClass: 'text-base-content/50' },
};

// Event type colors
export const EVENT_COLORS: Record<string, { bgClass: string; textClass: string }> = {
  agent:    { bgClass: 'bg-success/10', textClass: 'text-success' },
  workflow: { bgClass: 'bg-info/10',    textClass: 'text-info' },
  tool:     { bgClass: 'bg-warning/10', textClass: 'text-warning' },
  error:    { bgClass: 'bg-error/10',   textClass: 'text-error' },
};

// Workflow Builder: Node Catalog — static UI definitions
// Connectors and Agents sections populate dynamically from API data in NodeCatalog.svelte
export const NODE_CATALOG_ITEMS = [
  {
    category: 'Triggers',
    items: [
      { type: 'trigger-schedule', label: 'Schedule', desc: 'Run at set times', icon: '⏱' },
      { type: 'trigger-event', label: 'Event', desc: 'React to events', icon: '⚡' },
      { type: 'trigger-heartbeat', label: 'Heartbeat', desc: 'Run on interval', icon: '♥' },
      { type: 'trigger-manual', label: 'Manual', desc: 'Run on demand', icon: '▶' },
    ],
  },
  {
    category: 'Activities',
    items: [
      { type: 'activity-custom', label: 'Custom Activity', desc: 'Define steps and skills', icon: '◆' },
      { type: 'activity-research', label: 'Research', desc: 'Web search and analysis', icon: '⊕' },
      { type: 'activity-email', label: 'Send Email', desc: 'Compose and send email', icon: '✉' },
      { type: 'activity-notify', label: 'Notify', desc: 'Send notification', icon: '⊘' },
      { type: 'activity-code', label: 'Run Code', desc: 'Execute a code snippet', icon: '⌘' },
      { type: 'activity-http', label: 'HTTP Request', desc: 'Make an API call', icon: '⇄' },
      { type: 'activity-transform', label: 'Transform', desc: 'Reshape or filter data', icon: '⊿' },
    ],
  },
  {
    // Only shown when the builder is editing a call tree (and the groups
    // above are hidden there) — see NodeCatalog.svelte.
    category: 'Call Tree',
    items: [
      { type: 'activity-greeting', label: 'Greeting', desc: 'What the line says when it answers', icon: '☎' },
      { type: 'activity-intent', label: 'Intent', desc: 'One job + its allowed tools', icon: '⌁' },
      { type: 'activity-transfer', label: 'Transfer to human', desc: 'Live handoff to the transfer number', icon: '➦' },
      { type: 'activity-take_message', label: 'Take a message', desc: 'Capture a message (the fallback)', icon: '✉' },
    ],
  },
  {
    category: 'Flow Control',
    items: [
      { type: 'flow-condition', label: 'Condition', desc: 'If/else branching', icon: '⑂' },
      { type: 'flow-loop', label: 'Loop', desc: 'Iterate over items', icon: '↻' },
      { type: 'flow-wait', label: 'Wait', desc: 'Pause or wait for event', icon: '⏸' },
    ],
  },
  {
    category: 'Connectors (MCP)',
    items: [] as { type: string; label: string; desc: string; icon: string; serverId?: string; serverName?: string }[],
  },
  {
    category: 'Agents',
    items: [] as { type: string; label: string; desc: string; icon: string; agentId?: string; agentColor?: string }[],
  },
  {
    category: 'Output',
    items: [
      { type: 'emit', label: 'Emit Event', desc: 'Announce completion event', icon: '→' },
    ],
  },
];

// Workflow Builder: Architect Chat intro
export const ARCHITECT_INTRO_MESSAGE = {
  type: 'assistant' as const,
  content: 'I\'m the **Architect** — your workflow builder assistant.\n\nI can help you design and modify workflows. Try:\n- "Add an email notification step after the review"\n- "Create a new workflow that monitors GitHub issues"\n- "Change the trigger to run every 30 minutes"\n- "Connect the morning brief output to the content calendar"',
};
