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
import { opensHere } from './origin';
import { notifications, pushNotification, loadNotifications, settleUpdateNotices } from '$lib/stores/notifications';
import { askRaised, askSettled, loadOpenAsks } from '$lib/stores/permissionAsks';
import { loadWaitingAsks, setWaitingAsks, waitingAsks } from '$lib/stores/waitingAsks';
import { newBlockingAsks, seedBlockingAsks, openAsk, answerById } from '$lib/stores/blockingAsks';
import type { WaitingAsk } from '$lib/api/neboComponents';
import { addToast, removeToast } from '$lib/stores/toast';
import { onUpdateAvailable, onUpdateProgress, onUpdateReady, onUpdateError } from '$lib/stores/update';
import { logger } from '$lib/monitoring';
import { goto } from '$lib/nav';

const log = logger.child({ component: 'WSListeners' });

/** Chrome Web Store listing for the Nebo Browser Relay extension. */
const CHROME_EXTENSION_URL =
  'https://chromewebstore.google.com/detail/nebo-browser-relay/heaeiepdllbncnnlfniglgmbfmmemkcg';

let attached = false;
const unsubs: (() => void)[] = [];

// Notification ids already surfaced as a native OS notification this session — so a
// repeated broadcast of the same notification (e.g. a watcher re-emitting) doesn't
// fire a second banner.
const shownNotifIds = new Set<string>();

/** The waiting blocking ask an Inbox row (`permission-ask:<id>`) is for. */
function blockingAskOf(notificationId: string): WaitingAsk | undefined {
  const id = notificationId.startsWith('permission-ask:') ? notificationId.slice('permission-ask:'.length) : '';
  return id ? get(waitingAsks).find((a) => a.id === id && a.blocking) : undefined;
}

/** A blocking ask reaches the owner wherever he is in the app, never inside
 *  the chat he is in: a toast that fades on its own, and the OS notification.
 *  A click on either opens its card over the current screen. */
function tellBlockingAsk(ask: WaitingAsk): void {
  const title = get(t)('chat.waitingOnYou', { values: { name: ask.employee } });
  addToast(`${title}: ${ask.question}`, 'warning', 10000, {
    label: get(t)('chat.waitingOpen'),
    onClick: () => openAsk(ask.id),
  });
  void (async () => {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    if (!(window as any).__TAURI_INTERNALS__) return;
    try {
      const notif = await import('@tauri-apps/plugin-notification');
      let granted = await notif.isPermissionGranted();
      if (!granted) granted = (await notif.requestPermission()) === 'granted';
      if (!granted) return;
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('show_owner_notification', {
        title,
        body: ask.question,
        link: '',
        ask: ask.id,
        answers: ask.answerable ? ask.options : [],
      });
    } catch (e) {
      log.debug(`blocking ask notification not shown: ${String(e)}`);
    }
  })();
}

export function attachWebSocketListeners(): void {
  if (attached) return;
  attached = true;

  const ws = getWebSocketClient();

  // Bootstrap existing notifications (auth is ready at this point)
  loadNotifications();
  void loadOpenAsks().catch(() => log.debug('Asks API unavailable'));

  // --- Permission asks: one card everywhere; the first answer anywhere
  // clears it everywhere. ---
  unsubs.push(ws.on('permission_ask', (data: any) => askRaised(data)));
  unsubs.push(ws.on('permission_ask_resolved', (data: any) => askSettled(data)));

  // --- Everything waiting on the owner's answer, from every employee: the
  // pinned bar's list, loaded once and replaced whole on every change. ---
  void loadWaitingAsks()
    .then(seedBlockingAsks)
    .catch(() => log.debug('Waiting asks API unavailable'));
  unsubs.push(
    ws.on('asks_waiting', (data: any) => {
      setWaitingAsks(data);
      for (const ask of newBlockingAsks(data?.asks ?? [])) tellBlockingAsk(ask);
    })
  );

  // --- Notifications: store + toast ---
  unsubs.push(
    ws.on('notification', (data: any) => {
      log.debug('WS notification received');
      // ONE insert pathway: the store's pushNotification upserts by id, so a
      // WS re-broadcast can never duplicate a row the REST load already holds
      // (duplicate keys crash the Inbox's keyed each).
      const n = {
        id: data.id || `ws-${Date.now()}`,
        type: data.type || undefined,
        title: data.title || '',
        body: data.message || data.body || '',
        actionUrl: data.link || data.actionUrl || undefined,
        agentId: data.agentId || undefined,
      };
      pushNotification(n);
      // A blocking ask brought back as a reminder: its own notice, whose
      // click opens the card over the screen the owner is on.
      const blocking = blockingAskOf(n.id);
      if (blocking) {
        tellBlockingAsk(blocking);
        return;
      }
      addToast(n.title || n.body, n.type === 'error' ? 'error' : 'info');

      // Desktop: a NATIVE notification — system-wide (shows over any app),
      // auto-dismisses, and carries Nebo's icon. No webview window, so it cannot hang
      // the app (an always-on-top transparent window did). A click on it opens the
      // item's place, the same one its Inbox row and its phone push open (the
      // `owner-item-open` listener below). No-ops on the web build.
      void (async () => {
        if (shownNotifIds.has(n.id)) return; // dedupe repeated broadcasts of the same id
        shownNotifIds.add(n.id);
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        if (!(window as any).__TAURI_INTERNALS__) return;
        try {
          const notif = await import('@tauri-apps/plugin-notification');
          let granted = await notif.isPermissionGranted();
          if (!granted) granted = (await notif.requestPermission()) === 'granted';
          if (granted) {
            const { invoke } = await import('@tauri-apps/api/core');
            await invoke('show_owner_notification', {
              title: n.title || 'Nebo',
              body: n.body || '',
              // Every owner item names where it opens (the server's one link builder).
              link: n.actionUrl ?? '',
            });
          }
        } catch (e) {
          log.debug(`native notification not shown: ${String(e)}`);
        }
      })();
    })
  );

  // A click on a native notification opens the item's place in the main window;
  // on a blocking ask's, its card over the screen the owner is on, and its
  // action buttons (where the OS has them) answer it there and then.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  if (typeof window !== 'undefined' && (window as any).__TAURI_INTERNALS__) {
    void import('@tauri-apps/api/event')
      .then(async ({ listen }) => {
        unsubs.push(await listen<string>('owner-item-open', (e) => { if (e.payload) void goto(e.payload); }));
        unsubs.push(await listen<string>('owner-ask-open', (e) => { if (e.payload) openAsk(e.payload); }));
        unsubs.push(
          await listen<{ ask: string; index: number }>('owner-ask-answer', (e) => {
            if (e.payload?.ask) void answerById(e.payload.ask, e.payload.index);
          })
        );
      })
      .catch(() => log.debug('owner notification listeners not attached'));
  }

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
        addToast(`${data.agentName || get(t)('common.employee')}: ${data.message || get(t)('common.errorOccurred')}`, 'error');
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
  // Components only track connect *state* via ws.on. The server broadcasts to
  // every connected client, but only the client that started the sign-in opens
  // it: a second Nebo window (an old one left open after a restart) opened the
  // same sign-in twice on 2026-09-06, and the phone's web app would open the
  // desktop's (./origin.ts).
  unsubs.push(
    ws.on('plugin_auth_url', (data: any) => {
      if (typeof window === 'undefined' || !data?.url) return;
      if (!opensHere(data, 'nowhere')) return;
      // A blocked popup returns null. Silence here read as "Connect does
      // nothing" — say so instead.
      if (!window.open(data.url, '_blank')) {
        addToast(get(t)('agentSettings.signInPopupBlocked'), 'error', 8000);
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
      if (data.level === 'error') addToast(data.message || get(t)('common.systemError'), 'error');
    })
  );

  // --- Connection status toast ---
  // A dropped socket is routine on a phone (backgrounding, wifi↔cell) and
  // heals in a second or two. Say nothing for the first few seconds; then a
  // quiet "Reconnecting…" that goes away on its own the moment we are back;
  // only after half a minute does it become worth the owner's attention.
  let reconnectToast: number | null = null;
  let reconnectSoon: ReturnType<typeof setTimeout> | null = null;
  let reconnectLong: ReturnType<typeof setTimeout> | null = null;
  const clearReconnect = () => {
    if (reconnectSoon) { clearTimeout(reconnectSoon); reconnectSoon = null; }
    if (reconnectLong) { clearTimeout(reconnectLong); reconnectLong = null; }
    if (reconnectToast !== null) { removeToast(reconnectToast); reconnectToast = null; }
  };
  unsubs.push(
    ws.onStatus((status) => {
      if (status === 'connected') {
        clearReconnect();
        return;
      }
      if ((status === 'error' || status === 'disconnected') && !reconnectSoon && reconnectToast === null) {
        reconnectSoon = setTimeout(() => {
          reconnectSoon = null;
          if (ws.getStatus() === 'connected') return;
          reconnectToast = addToast(get(t)('common.reconnecting'), 'info', 0);
          reconnectLong = setTimeout(() => {
            if (ws.getStatus() === 'connected') return;
            if (reconnectToast !== null) removeToast(reconnectToast);
            reconnectToast = addToast(get(t)('common.stillReconnecting'), 'warning', 0);
          }, 30_000);
        }, 4_000);
      }
    })
  );
  unsubs.push(clearReconnect);

  // --- Artifact update toasts ---
  unsubs.push(
    ws.on('artifact_updates_available', (data: any) => {
      if (data.count > 0) {
        addToast(get(t)('common.updatesAvailable', { values: { count: data.count } }), 'info');
      }
      for (const u of (data.updates ?? []) as Array<{ type: string; id: string; remoteVersion: string }>) {
        settleUpdateNotices(u.type, u.id, u.remoteVersion);
      }
    })
  );
  unsubs.push(
    ws.on('artifact_update_applied', (data: any) => {
      addToast(get(t)('common.artifactUpdated', { values: { type: data.type, version: data.version } }), 'success');
      settleUpdateNotices(data.type, data.id);
    })
  );
  unsubs.push(
    ws.on('artifact_update_failed', (data: any) => {
      addToast(get(t)('common.artifactUpdateFailed', { values: { error: data.error } }), 'error');
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
