/**
 * Nebo Browser Relay - Tab Management
 */

import type { AttachedTab, CDPTargetInfo } from './types.js'
import * as badge from './badge.js'

// Tab state tracking
const tabs = new Map<number, AttachedTab>()
const sessionToTab = new Map<string, number>()
const childSessions = new Map<string, number>()

let sessionCounter = 1

export function generateSessionId(): string {
  return `nebo-${sessionCounter++}`
}

export function getTab(tabId: number): AttachedTab | undefined {
  return tabs.get(tabId)
}

export function isAttached(tabId: number): boolean {
  const tab = tabs.get(tabId)
  return tab?.state === 'connected'
}

export function getTabBySession(sessionId: string): { tabId: number; isChild: boolean } | null {
  const direct = sessionToTab.get(sessionId)
  if (direct !== undefined) {
    return { tabId: direct, isChild: false }
  }

  const child = childSessions.get(sessionId)
  if (child !== undefined) {
    return { tabId: child, isChild: true }
  }

  return null
}

export function getTabByTarget(targetId: string): number | null {
  for (const [tabId, tab] of tabs.entries()) {
    if (tab.targetId === targetId) {
      return tabId
    }
  }
  return null
}

export function getFirstConnectedTab(): number | null {
  for (const [tabId, tab] of tabs.entries()) {
    if (tab.state === 'connected') {
      return tabId
    }
  }
  return null
}

export function getAllTabs(): Map<number, AttachedTab> {
  return tabs
}

export function setConnecting(tabId: number, port: number): void {
  tabs.set(tabId, { state: 'connecting', port })
  badge.showConnecting(tabId)
}

export function setConnected(
  tabId: number,
  sessionId: string,
  targetId: string,
  port: number
): void {
  tabs.set(tabId, {
    state: 'connected',
    sessionId,
    targetId,
    attachOrder: sessionCounter,
    port
  })
  sessionToTab.set(sessionId, tabId)
  badge.showAttached(tabId)
}

export function registerChildSession(sessionId: string, tabId: number): void {
  childSessions.set(sessionId, tabId)
}

export function unregisterChildSession(sessionId: string): void {
  childSessions.delete(sessionId)
}

export function removeTab(tabId: number): AttachedTab | undefined {
  const tab = tabs.get(tabId)
  if (tab) {
    if (tab.sessionId) {
      sessionToTab.delete(tab.sessionId)
    }
    tabs.delete(tabId)

    // Clean up child sessions for this tab
    for (const [childId, parentId] of childSessions.entries()) {
      if (parentId === tabId) {
        childSessions.delete(childId)
      }
    }

    badge.showDetached(tabId)
  }
  return tab
}

export function clearAllTabs(): void {
  for (const tabId of tabs.keys()) {
    badge.showDisconnected(tabId)
  }
  tabs.clear()
  sessionToTab.clear()
  childSessions.clear()
}

export async function attachToTab(tabId: number): Promise<{
  sessionId: string
  targetId: string
  targetInfo: CDPTargetInfo
}> {
  const debuggee = { tabId }

  // Attach debugger
  await chrome.debugger.attach(debuggee, '1.3')
  await chrome.debugger.sendCommand(debuggee, 'Page.enable').catch(() => {})

  // Get target info
  const result = await chrome.debugger.sendCommand(debuggee, 'Target.getTargetInfo') as {
    targetInfo?: CDPTargetInfo
  }

  const targetInfo = result?.targetInfo
  const targetId = targetInfo?.targetId?.trim()

  if (!targetId || !targetInfo) {
    throw new Error('Failed to get target ID from Chrome')
  }

  const sessionId = generateSessionId()

  // Add attached flag and ensure browserContextId is present
  // Playwright requires a truthy browserContextId - use a consistent default
  const fullTargetInfo: CDPTargetInfo = {
    ...targetInfo,
    attached: true,
    browserContextId: targetInfo.browserContextId || 'default'
  }

  return { sessionId, targetId, targetInfo: fullTargetInfo }
}

export async function detachFromTab(tabId: number): Promise<void> {
  try {
    await chrome.debugger.detach({ tabId })
  } catch {
    // Tab may already be detached
  }
}
