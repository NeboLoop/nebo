// Slash command registry — categories and definitions.

export interface SlashCommand {
  name: string;
  category: string;
  desc: string;
  local: boolean;
  args?: string;
}

export interface CommandGroup {
  category: string;
  items: SlashCommand[];
}

export const SLASH_COMMANDS: SlashCommand[] = [
  // Session
  { name: '/new',      category: 'Session', desc: 'Start a new thread',              local: true },
  { name: '/clear',    category: 'Session', desc: 'Clear chat display',              local: true },
  { name: '/stop',     category: 'Session', desc: 'Stop current generation',         local: true },
  { name: '/compact',  category: 'Session', desc: 'Summarize & clear old messages',  local: false },

  // Model
  { name: '/model',    category: 'Model', desc: 'Show or switch model',              local: false, args: '[name]' },
  { name: '/think',    category: 'Model', desc: 'Set thinking mode',                 local: false, args: 'off|low|medium|high' },
  { name: '/verbose',  category: 'Model', desc: 'Toggle tool output detail',         local: true,  args: 'on|off' },

  // Info
  { name: '/help',     category: 'Info', desc: 'Show all commands',                  local: true },
  { name: '/status',   category: 'Info', desc: 'Show agent & system status',         local: false },
  { name: '/usage',    category: 'Info', desc: 'Show credit usage',                  local: false },
  { name: '/export',   category: 'Info', desc: 'Export chat as Markdown',            local: true },
  { name: '/search',   category: 'Info', desc: 'Search chat history',               local: true,  args: '<query>' },

  // Agent
  { name: '/skill',    category: 'Agent', desc: 'Activate a skill by name',          local: false, args: '<name>' },
  { name: '/memory',   category: 'Agent', desc: 'Search or list memories',           local: false, args: '[query]' },
  { name: '/advisors', category: 'Agent', desc: 'List active advisors',              local: false },
  { name: '/personality', category: 'Agent', desc: 'Show current personality',       local: false },
];

/**
 * Filter commands by prefix query (e.g. "/he" matches "/help").
 * Returns grouped array of matching commands.
 */
export function filterCommands(query: string): CommandGroup[] {
  const q = query.toLowerCase();
  const matching = SLASH_COMMANDS.filter(c => c.name.startsWith(q));

  const groups: CommandGroup[] = [];
  const seen = new Set<string>();
  for (const cmd of matching) {
    if (!seen.has(cmd.category)) {
      seen.add(cmd.category);
      groups.push({ category: cmd.category, items: [] });
    }
    groups.find(g => g.category === cmd.category)!.items.push(cmd);
  }
  return groups;
}
