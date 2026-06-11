/**
 * Download a Work-panel artifact. In a browser the anchor's `download`
 * attribute handles it — this is a no-op. In the Tauri desktop build,
 * WKWebView ignores that attribute, so we intercept the click and save
 * natively to ~/Downloads (revealing the file in the file manager).
 */
export async function downloadArtifact(e: MouseEvent, fileUrl: string): Promise<void> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  if (!(window as any).__TAURI_INTERNALS__) return;
  e.preventDefault();
  const fileName = decodeURIComponent(fileUrl.split('/').pop() || '');
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('save_artifact', { fileName });
  } catch (err) {
    console.error('save artifact failed', err);
  }
}
