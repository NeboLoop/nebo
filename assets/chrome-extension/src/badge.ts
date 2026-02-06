/**
 * Nebo Browser Relay - Badge UI
 */

import { BADGE_STATES, type BadgeState } from './types.js'

export function setBadge(tabId: number, state: BadgeState): void {
  const config = BADGE_STATES[state]

  void chrome.action.setBadgeText({ tabId, text: config.text })
  void chrome.action.setBadgeBackgroundColor({ tabId, color: config.color })
  void chrome.action.setBadgeTextColor({ tabId, color: '#FFFFFF' }).catch(() => {
    // setBadgeTextColor may not be available in all Chrome versions
  })
}

export function setTitle(tabId: number, message: string): void {
  void chrome.action.setTitle({
    tabId,
    title: `Nebo: ${message}`
  })
}

export function showAttached(tabId: number): void {
  setBadge(tabId, 'on')
  setTitle(tabId, 'attached (click to detach)')
}

export function showDetached(tabId: number): void {
  setBadge(tabId, 'off')
  setTitle(tabId, 'click to attach')
}

export function showConnecting(tabId: number): void {
  setBadge(tabId, 'connecting')
  setTitle(tabId, 'connecting to relay…')
}

export function showDisconnected(tabId: number): void {
  setBadge(tabId, 'connecting')
  setTitle(tabId, 'disconnected (click to re-attach)')
}

export function showError(tabId: number, reason?: string): void {
  setBadge(tabId, 'error')
  setTitle(tabId, reason ?? 'relay not running (open options for setup)')
}
