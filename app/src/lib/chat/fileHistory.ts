/**
 * A workspace file's earlier versions in the Work panel. Every file in the
 * owner's workspace keeps what it was before any change or delete, by any
 * means (`tools::workspace_history` on the bot); a file opened by its path
 * lists them under "History" and puts one back with Restore. The bot keeps
 * what the file is now before a restore, so a restore is undone the same way.
 */

/** One earlier version, as `GET /api/v1/work/history` gives it. */
export interface FileHistoryItem {
  id: number;
  /** Where its kept bytes are served (`/api/v1/files/work/blobs/…`). */
  url: string;
  sizeBytes: number;
  /** `modified` or `deleted`. */
  reason: string;
  /** Unix seconds. */
  capturedAt: number;
}

const FILES_ROUTE = "/api/v1/files/";

/** The file's place in the workspace, from the URL it is opened by
 *  (`/api/v1/files/<encoded path>`); null for anything else. */
export function workspacePathOf(url: string): string | null {
  if (!url.startsWith(FILES_ROUTE)) return null;
  const rest = url.slice(FILES_ROUTE.length).split(/[?#]/)[0];
  if (!rest) return null;
  try {
    return rest.split("/").map(decodeURIComponent).join("/");
  } catch {
    return null;
  }
}

/** The entries a history answer carries, newest first; nothing for a
 *  malformed one. */
export function historyItems(
  answer: { entries?: unknown[] } | null | undefined,
): FileHistoryItem[] {
  return (answer?.entries ?? []).filter(
    (e): e is FileHistoryItem =>
      !!e &&
      typeof (e as FileHistoryItem).id === "number" &&
      typeof (e as FileHistoryItem).url === "string",
  );
}
