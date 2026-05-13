/**
 * WebSocket Event Listeners
 *
 * Subscribes to real-time WebSocket events and updates Svelte stores.
 * Call `attachWebSocketListeners()` once after the WebSocket connects.
 */

import { getWebSocketClient } from './client';
import { notifications, pushNotification, loadNotifications } from '$lib/stores/notifications';
import { addToast } from '$lib/stores/toast';
import { logger } from '$lib/monitoring';

const log = logger.child({ component: 'WSListeners' });

let attached = false;
const unsubs: (() => void)[] = [];

export function attachWebSocketListeners(): void {
  if (attached) return;
  attached = true;

  const ws = getWebSocketClient();

  // Bootstrap existing notifications (auth is ready at this point)
  loadNotifications();

  // --- Notifications ---
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
    })
  );

  // --- Chat streaming ---
  unsubs.push(
    ws.on('chat_stream', (data: any) => {
      // Dispatch a custom DOM event that chat components can listen to
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:chat_stream', { detail: data }));
      }
    })
  );

  unsubs.push(
    ws.on('chat_message', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:chat_message', { detail: data }));
      }
    })
  );

  // --- Agent status changes ---
  unsubs.push(
    ws.on('agent_status', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:agent_status', { detail: data }));
      }
      if (data.status === 'error') {
        addToast(`${data.agentName || 'Agent'}: ${data.message || 'Error occurred'}`, 'error');
      }
    })
  );

  // --- Approval requests ---
  unsubs.push(
    ws.on('approval_request', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:approval_request', { detail: data }));
      }
      addToast(`${data.agentName || 'Agent'} needs approval`, 'warning');
    })
  );

  // --- Ask requests (interactive prompts) ---
  unsubs.push(
    ws.on('ask_request', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:ask_request', { detail: data }));
      }
    })
  );

  // --- Run status updates ---
  unsubs.push(
    ws.on('run_update', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:run_update', { detail: data }));
      }
    })
  );

  // --- Workflow run updates ---
  unsubs.push(
    ws.on('workflow_update', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:workflow_update', { detail: data }));
      }
    })
  );

  // --- Agent lifecycle ---
  for (const evt of ['agent_activated', 'agent_deactivated', 'agent_installed', 'agent_uninstalled', 'agent_updated'] as const) {
    unsubs.push(
      ws.on(evt, (data: any) => {
        if (typeof window !== 'undefined') {
          window.dispatchEvent(new CustomEvent(`nebo:${evt}`, { detail: data }));
        }
      })
    );
  }

  // --- Chat lifecycle ---
  unsubs.push(
    ws.on('chat_complete', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:chat_complete', { detail: data }));
      }
    })
  );

  unsubs.push(
    ws.on('chat_title_updated', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:chat_title_updated', { detail: data }));
      }
    })
  );

  unsubs.push(
    ws.on('chat_created', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:chat_created', { detail: data }));
      }
    })
  );

  // --- Tool execution ---
  unsubs.push(
    ws.on('tool_start', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:tool_start', { detail: data }));
      }
    })
  );

  unsubs.push(
    ws.on('tool_result', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:tool_result', { detail: data }));
      }
    })
  );

  unsubs.push(
    ws.on('thinking', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:thinking', { detail: data }));
      }
    })
  );

  // --- Subagent ---
  for (const evt of ['subagent_start', 'subagent_progress', 'subagent_complete'] as const) {
    unsubs.push(
      ws.on(evt, (data: any) => {
        if (typeof window !== 'undefined') {
          window.dispatchEvent(new CustomEvent(`nebo:${evt}`, { detail: data }));
        }
      })
    );
  }

  // --- Workflow run lifecycle ---
  for (const evt of ['workflow_run_started', 'workflow_run_completed', 'workflow_run_failed', 'workflow_activity_update'] as const) {
    unsubs.push(
      ws.on(evt, (data: any) => {
        if (typeof window !== 'undefined') {
          window.dispatchEvent(new CustomEvent(`nebo:${evt}`, { detail: data }));
        }
      })
    );
  }

  // --- Notification created (push directly into store) ---
  unsubs.push(
    ws.on('notification_created', (data: any) => {
      pushNotification(data);
    })
  );

  // --- Task item updates (per-step workflow progress) ---
  unsubs.push(
    ws.on('task_updated', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:task_updated', { detail: data }));
      }
    })
  );

  // --- A2UI data model updates ---
  unsubs.push(
    ws.on('a2ui_update_data_model', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:a2ui_data', { detail: data }));
      }
    })
  );

  unsubs.push(
    ws.on('a2ui_action_status', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:a2ui_action_status', { detail: data }));
      }
    })
  );

  // --- Plugin auth lifecycle ---
  for (const evt of ['plugin_auth_started', 'plugin_auth_url', 'plugin_auth_complete', 'plugin_auth_error', 'agent_auth_required'] as const) {
    unsubs.push(
      ws.on(evt, (data: any) => {
        if (typeof window !== 'undefined') {
          window.dispatchEvent(new CustomEvent(`nebo:${evt}`, { detail: data }));
        }
      })
    );
  }

  // --- Plan changed ---
  unsubs.push(
    ws.on('plan_changed', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:plan_changed', { detail: data }));
      }
    })
  );

  // --- System events ---
  unsubs.push(
    ws.on('system_event', (data: any) => {
      if (data.level === 'error') {
        addToast(data.message || 'System error', 'error');
      }
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

  log.info('WebSocket listeners attached');
}

export function detachWebSocketListeners(): void {
  for (const unsub of unsubs) {
    unsub();
  }
  unsubs.length = 0;
  attached = false;
  log.debug('WebSocket listeners detached');
}
