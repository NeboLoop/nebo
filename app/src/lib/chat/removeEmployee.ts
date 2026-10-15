// Removing an employee from a roster the owner is looking at: the row goes
// the moment the owner confirms, and the server is asked after. A refusal
// puts the row back where it was. The server's own answer (its
// `agent_uninstalled` broadcast) then reloads the roster once.

export type RosterRow = { id: string };

/**
 * Drop `id` from the roster now (`set` with the rows left), then run
 * `remove`. When `remove` fails, `set` gets the roster as it was and the
 * error is rethrown for the caller to say so.
 */
export async function removeEmployee<T extends RosterRow>(
  rows: T[],
  id: string,
  set: (rows: T[]) => void,
  remove: () => Promise<unknown>,
): Promise<void> {
  set(rows.filter((r) => r.id !== id));
  try {
    await remove();
  } catch (e) {
    set(rows);
    throw e;
  }
}
