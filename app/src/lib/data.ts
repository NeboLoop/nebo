// Bidirectional mapping: full agent IDs (API) ↔ short IDs (tokens/calendar)
export const AGENT_ID_MAP: Record<string, string> = {
  researcher: 'res', coder: 'cod', marketer: 'mkt',
  social: 'soc', tester: 'tst', ops: 'ops',
  assistant: 'ast', writer: 'wrt',
};
export const AGENT_ID_REVERSE: Record<string, string> =
  Object.fromEntries(Object.entries(AGENT_ID_MAP).map(([k, v]) => [v, k]));

export const AGENTS: { id: string; initial: string; name: string; role: string }[] = [];

export const CAL_DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
