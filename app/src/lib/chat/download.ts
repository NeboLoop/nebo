/**
 * Download a Work-panel artifact. In a browser the anchor's `download`
 * attribute handles it — this is a no-op. In the Tauri desktop build,
 * WKWebView ignores that attribute, so we intercept the click and save
 * natively to ~/Downloads (revealing the file in the file manager).
 */
export async function downloadArtifact(e: MouseEvent, fileUrl: string, saveName?: string): Promise<void> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  if (!(window as any).__TAURI_INTERNALS__) return;
  e.preventDefault();
  // Path within the server's files dir, e.g. "work/blobs/<hash>.md" — the blob
  // lives there, not at the bare last segment. Strip origin, /files/ prefix, query.
  const path = fileUrl.split('?')[0];
  const relPath = decodeURIComponent(path.split('/files/').pop() || path.split('/').pop() || '');
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('save_artifact', { relPath, saveName: saveName || relPath.split('/').pop() || '' });
  } catch (err) {
    console.error('save artifact failed', err);
  }
}
