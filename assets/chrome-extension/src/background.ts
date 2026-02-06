/**
 * Nebo Browser Relay - Service Worker Entry Point
 *
 * Bridges Chrome tabs to Nebo's browser automation via CDP relay.
 * Click the toolbar icon to attach/detach tabs.
 */

import * as relay from './relay.js'
import * as tabs from './tabs.js'
import * as badge from './badge.js'
import { getActivePort } from './storage.js'

async function handleToolbarClick(): Promise<void> {
  const [active] = await chrome.tabs.query({ active: true, currentWindow: true })
  const tabId = active?.id

  if (!tabId) return

  // Toggle if already attached
  if (tabs.isAttached(tabId)) {
    await detachTab(tabId)
    return
  }

  // Attach tab
  await attachTab(tabId)
}

async function attachTab(tabId: number): Promise<void> {
  const port = await getActivePort()

  tabs.setConnecting(tabId, port)

  try {
    // Ensure relay connection
    await relay.ensureConnection()

    // Attach debugger to tab
    const { sessionId, targetId, targetInfo } = await tabs.attachToTab(tabId)

    // Update state
    tabs.setConnected(tabId, sessionId, targetId, port)

    // Notify relay with full target info (title, url, type, etc.)
    relay.send({
      method: 'forwardCDPEvent',
      params: {
        method: 'Target.attachedToTarget',
        params: {
          sessionId,
          targetInfo,
          waitingForDebugger: false
        }
      }
    })

  } catch (err) {
    tabs.removeTab(tabId)
    badge.showError(tabId)

    console.warn('[nebo] attach failed:', err instanceof Error ? err.message : err)

    await relay.showHelpOnFirstError()
  }
}

async function detachTab(tabId: number): Promise<void> {
  const tab = tabs.getTab(tabId)

  // Notify relay of detachment
  if (tab?.sessionId && tab?.targetId) {
    try {
      relay.send({
        method: 'forwardCDPEvent',
        params: {
          method: 'Target.detachedFromTarget',
          params: {
            sessionId: tab.sessionId,
            targetId: tab.targetId,
            reason: 'user'
          }
        }
      })
    } catch {
      // Ignore
    }
  }

  // Detach debugger
  await tabs.detachFromTab(tabId)
  tabs.removeTab(tabId)
}

// Event handlers
chrome.action.onClicked.addListener(() => {
  void handleToolbarClick()
})

chrome.runtime.onInstalled.addListener(() => {
  // Show options page on first install
  void chrome.runtime.openOptionsPage()

  // Set up keep-alive alarm to prevent service worker suspension
  void chrome.alarms.create('keep-alive', { periodInMinutes: 0.4 }) // ~24 seconds
})

// Keep-alive alarm handler - prevents service worker suspension while connected
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === 'keep-alive') {
    // If we have connected tabs, ensure relay stays connected
    const connectedTabs = tabs.getAllTabs()
    if (connectedTabs.size > 0 && !relay.isConnected()) {
      console.log('[nebo] Reconnecting relay after service worker wake...')
      void relay.ensureConnection().catch((err) => {
        console.warn('[nebo] Failed to reconnect:', err)
      })
    }
  }
})

// Also ensure alarm exists on startup (in case extension was updated)
void chrome.alarms.get('keep-alive').then((alarm) => {
  if (!alarm) {
    void chrome.alarms.create('keep-alive', { periodInMinutes: 0.4 })
  }
})

// Export for debugging
Object.assign(globalThis, { nebo: { relay, tabs, badge } })
