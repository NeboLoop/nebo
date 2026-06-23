/**
 * WebSocket app-global SIDE EFFECTS.
 *
 * This is the single owner of cross-cutting reactions to WS events — toasts, the
 * notification store, the app-update store, and opening a plugin OAuth URL. It is
 * NOT an event bridge: components subscribe to WS events directly through the
 * `ws.on` emitter (see `lib/websocket/subscribe.ts` → `onWsEvent`), so each event
 * drives a given side effect from exactly ONE place (CODE_AUDITOR Rule 8). There
 * is no `window`-CustomEvent re-dispatch anymore.
 *
 * Call `attachWebSocketListeners()` once after the WebSocket connects.
 */

import { get } from 'svelte/store';
import { t } from 'svelte-i18n';
import { getWebSocketClient } from './client';
import { notifications, pushNotification, loadNotifications } from '$lib/stores/notifications';
import { addToast } from '$lib/stores/toast';
import { onUpdateAvailable, onUpdateProgress, onUpdateReady, onUpdateError } from '$lib/stores/update';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'WSListeners' });

/** Chrome Web Store listing for the Nebo Browser Relay extension. */
const CHROME_EXTENSION_URL =
  'https://chromewebstore.google.com/detail/nebo-browser-relay/heaeiepdllbncnnlfniglgmbfmmemkcg';

let attached = false;
const unsubs: (() => void)[] = [];

export function attachWebSocketListeners(): void {
  if (attached) return;
  attached = true;

  const ws = getWebSocketClient();

  // Bootstrap existing notifications (auth is ready at this point)
  loadNotifications();

  // --- Notifications: store + toast ---
  unsubs.push(
    ws.on('notification', (data: any) => {
      log.debug('WS notification received');
      const n = {
        id: data.id || `ws-${Date.now()}`,
        type: data.type || 'system',
        title: data.title || '',
        message: data.message || data.body || '',
        time: 'just now',
        read: false,
        link: data.link || data.actionUrl || undefined,
      };
      notifications.update(list => [n, ...list]);
      addToast(n.title || n.message, n.type === 'error' ? 'error' : 'info');

      // Desktop: surface a branded, auto-dismissing HUD (replaces the osascript
      // modal). No-ops on the web build where the Tauri import fails.
      void (async () => {
        try {
          const { invoke } = await import('@tauri-apps/api/core');
          const KIND: Record<string, string> = {
            agent: 'message',
            warning: 'alert',
            error: 'alert',
            system: 'reminder',
          };
          await invoke('show_notification', {
            title: n.title || 'Nebo',
            body: n.message || '',
            agent: data.agent || data.agentName || undefined,
            kind: data.kind || KIND[n.type] || 'reminder',
            time: data.time || undefined,
            accent: data.accent || undefined,
          });
        } catch {
          /* web build — no Tauri runtime */
        }
      })();
    })
  );

  unsubs.push(
    ws.on('notification_created', (data: any) => {
      pushNotification(data);
    })
  );

  // --- Browser extension: nudge the user to install it when it's missing ---
  // Tier-1 (authenticated) browser; research falls back to built-in Chrome when
  // absent. Research fan-out can fire this many times, so rate-limit.
  let lastExtPrompt = 0;
  unsubs.push(
    ws.on('browser_extension_disconnected', (data: any) => {
      if (data?.reason === 'reconnecting') return; // transient — don't nag
      const now = Date.now();
      if (now - lastExtPrompt < 10 * 60 * 1000) return; // at most once per 10 min
      lastExtPrompt = now;
      const msg = `${get(t)('browserExtension.notConnected')} ${get(t)('browserExtension.instructions')}`;
      addToast(msg, 'warning', 12000, {
        label: get(t)('browserExtension.install'),
        url: CHROME_EXTENSION_URL,
      });
    })
  );

  // --- Error / attention toasts ---
  unsubs.push(
    ws.on('agent_status', (data: any) => {
      if (data.status === 'error') {
        addToast(`${data.agentName || 'Agent'}: ${data.message || 'Error occurred'}`, 'error');
      }
    })
  );

  // `approval_request` is handled by <ApprovalGate/> (root layout), which shows
  // the actionable ApprovalModal and sends `approval_response`. No toast here —
  // the modal is the single, actionable signal.

  unsubs.push(
    ws.on('quota_warning', (data: any) => {
      if (data?.text) addToast(data.text, 'warning');
    })
  );

  // --- Plugin OAuth: open the auth URL once, app-wide (the single owner). ---
  // Always open — auth can be triggered by agent startup/watchers when no
  // page-level UI is mounted. Components only track connect *state* via ws.on.
  unsubs.push(
    ws.on('plugin_auth_url', (data: any) => {
      if (typeof window !== 'undefined' && data?.url) {
        window.open(data.url, '_blank');
      }
    })
  );

  // --- App update lifecycle (update store + error toast) ---
  unsubs.push(ws.on('update_available', (data: any) => onUpdateAvailable(data)));
  unsubs.push(ws.on('update_progress', (data: any) => onUpdateProgress(data)));
  unsubs.push(ws.on('update_ready', (data: any) => onUpdateReady(data)));
  unsubs.push(
    ws.on('update_error', (data: any) => {
      onUpdateError(data);
      if (data?.error || data?.message) addToast(String(data.error || data.message), 'error');
    })
  );

  // --- System events ---
  unsubs.push(
    ws.on('system_event', (data: any) => {
      if (data.level === 'error') addToast(data.message || 'System error', 'error');
    })
  );

  // --- Connection status toast ---
  unsubs.push(
    ws.onStatus((status) => {
      if (status === 'error') {
        addToast('WebSocket connection lost. Reconnecting...', 'warning');
      }
    })
  );

  // --- Artifact update toasts ---
  unsubs.push(
    ws.on('artifact_updates_available', (data: any) => {
      if (data.count > 0) {
        addToast(`${data.count} update${data.count > 1 ? 's' : ''} available`, 'info');
      }
    })
  );
  unsubs.push(
    ws.on('artifact_update_applied', (data: any) => {
      addToast(`Updated ${data.type}: ${data.version}`, 'success');
    })
  );
  unsubs.push(
    ws.on('artifact_update_failed', (data: any) => {
      addToast(`Update failed: ${data.error}`, 'error');
    })
  );

  log.info('WebSocket side-effect listeners attached');
}

export function detachWebSocketListeners(): void {
  for (const unsub of unsubs) {
    unsub();
  }
  unsubs.length = 0;
  attached = false;
  log.debug('WebSocket listeners detached');
}
