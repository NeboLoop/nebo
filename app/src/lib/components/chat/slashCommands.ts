// Slash command registry — categories and definitions.
// All commands are handled by the backend. The frontend only provides
// the menu UI and sends the command text through the normal chat pipeline.

export interface SlashCommand {
  name: string;
  category: string;
  desc: string;
  args?: string;
}

export interface CommandGroup {
  category: string;
  items: SlashCommand[];
}

export const SLASH_COMMANDS: SlashCommand[] = [
  // Session
  { name: '/new',      category: 'Session', desc: 'Start a new conversation' },
  { name: '/clear',    category: 'Session', desc: 'Clear current conversation' },
  { name: '/compact',  category: 'Session', desc: 'Summarize & clear old messages' },

  // Model
  { name: '/model',    category: 'Model', desc: 'Show or switch model',              args: '[name]' },

  // Info
  { name: '/help',     category: 'Info', desc: 'Show all commands' },
  { name: '/status',   category: 'Info', desc: 'Show agent & system status' },
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
