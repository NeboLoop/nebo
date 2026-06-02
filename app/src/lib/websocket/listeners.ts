/**
 * WebSocket Event Listeners
 *
 * Subscribes to real-time WebSocket events and updates Svelte stores.
 * Call `attachWebSocketListeners()` once after the WebSocket connects.
 */

import { getWebSocketClient } from './client';
import { notifications, pushNotification, loadNotifications } from '$lib/stores/notifications';
import { addToast } from '$lib/stores/toast';
import { onUpdateAvailable, onUpdateProgress, onUpdateReady, onUpdateError } from '$lib/stores/update';
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

  // --- Follow-up suggestions ---
  unsubs.push(
    ws.on('followup_suggestions', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:followup_suggestions', { detail: data }));
      }
    })
  );

  // --- Plan approval ---
  unsubs.push(
    ws.on('plan_approval', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:plan_approval', { detail: data }));
      }
    })
  );

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

  // --- Code install lifecycle ---
  for (const evt of ['code_processing', 'code_result', 'plugin_installing', 'plugin_installed', 'dep_started', 'dep_pending', 'dep_installed', 'dep_failed', 'dep_cascade_complete'] as const) {
    unsubs.push(
      ws.on(evt, (data: any) => {
        if (typeof window !== 'undefined') {
          window.dispatchEvent(new CustomEvent(`nebo:${evt}`, { detail: data }));
        }
      })
    );
  }

  // --- Plugin auth lifecycle ---
  for (const evt of ['plugin_auth_started', 'plugin_auth_url', 'plugin_auth_complete', 'plugin_auth_error', 'agent_auth_required'] as const) {
    unsubs.push(
      ws.on(evt, (data: any) => {
        if (typeof window !== 'undefined') {
          window.dispatchEvent(new CustomEvent(`nebo:${evt}`, { detail: data }));
          // Always open OAuth URLs — page-level listeners may not be active
          // when auth is triggered by agent startup or watchers.
          if (evt === 'plugin_auth_url' && data?.url) {
            window.open(data.url, '_blank');
          }
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

  // --- Token usage ---
  unsubs.push(
    ws.on('usage', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:usage', { detail: data }));
      }
    })
  );

  // --- Quota warnings ---
  unsubs.push(
    ws.on('quota_warning', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:quota_warning', { detail: data }));
      }
      if (data?.text) {
        addToast(data.text, 'warning');
      }
    })
  );

  // --- Ghost text (inline completion) ---
  unsubs.push(
    ws.on('ghost_text', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:ghost_text', { detail: data }));
      }
    })
  );

  // --- App update lifecycle ---
  unsubs.push(
    ws.on('update_available', (data: any) => {
      onUpdateAvailable(data);
    })
  );

  unsubs.push(
    ws.on('update_progress', (data: any) => {
      onUpdateProgress(data);
    })
  );

  unsubs.push(
    ws.on('update_ready', (data: any) => {
      onUpdateReady(data);
    })
  );

  unsubs.push(
    ws.on('update_error', (data: any) => {
      onUpdateError(data);
      if (data?.error || data?.message) {
        addToast(String(data.error || data.message), 'error');
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

  // --- Artifact Updates ---
  unsubs.push(
    ws.on('artifact_updates_available', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:artifact_updates_available', { detail: data }));
      }
      if (data.count > 0) {
        addToast(`${data.count} update${data.count > 1 ? 's' : ''} available`, 'info');
      }
    })
  );

  unsubs.push(
    ws.on('artifact_update_applied', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:artifact_update_applied', { detail: data }));
      }
      addToast(`Updated ${data.type}: ${data.version}`, 'success');
    })
  );

  unsubs.push(
    ws.on('artifact_update_failed', (data: any) => {
      if (typeof window !== 'undefined') {
        window.dispatchEvent(new CustomEvent('nebo:artifact_update_failed', { detail: data }));
      }
      addToast(`Update failed: ${data.error}`, 'error');
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
