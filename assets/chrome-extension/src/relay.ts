/**
 * Nebo Browser Relay - WebSocket Connection
 */

import type { RelayMessage, PendingRequest } from './types.js'
import * as tabs from './tabs.js'
import * as badge from './badge.js'
import { getRelayUrl, getActivePort, hasShownHelp, markHelpShown } from './storage.js'

let socket: WebSocket | null = null
let connectPromise: Promise<void> | null = null
let debuggerListenersInstalled = false

const pending = new Map<number, PendingRequest>()

export function isConnected(): boolean {
  return socket !== null && socket.readyState === WebSocket.OPEN
}

export async function ensureConnection(): Promise<void> {
  if (isConnected()) return
  if (connectPromise) return await connectPromise

  connectPromise = connect()

  try {
    await connectPromise
  } finally {
    connectPromise = null
  }
}

async function connect(): Promise<void> {
  const { http: httpUrl, ws: wsUrl } = await getRelayUrl()

  // Preflight check - is relay running?
  try {
    await fetch(httpUrl, {
      method: 'HEAD',
      signal: AbortSignal.timeout(2000)
    })
  } catch (err) {
    throw new Error(`Relay not reachable at ${httpUrl}`)
  }

  // Connect WebSocket
  const ws = new WebSocket(wsUrl)
  socket = ws

  await new Promise<void>((resolve, reject) => {
    const timeout = setTimeout(() => {
      reject(new Error('WebSocket connection timeout'))
    }, 5000)

    ws.onopen = () => {
      clearTimeout(timeout)
      resolve()
    }

    ws.onerror = () => {
      clearTimeout(timeout)
      reject(new Error('WebSocket connection failed'))
    }

    ws.onclose = (ev) => {
      clearTimeout(timeout)
      reject(new Error(`WebSocket closed: ${ev.code} ${ev.reason || ''}`))
    }
  })

  // Set up message handling
  ws.onmessage = (event) => handleMessage(String(event.data || ''))
  ws.onclose = () => handleDisconnect('closed')
  ws.onerror = () => handleDisconnect('error')

  // Install debugger listeners once
  if (!debuggerListenersInstalled) {
    debuggerListenersInstalled = true
    chrome.debugger.onEvent.addListener(handleDebuggerEvent as Parameters<typeof chrome.debugger.onEvent.addListener>[0])
    chrome.debugger.onDetach.addListener(handleDebuggerDetach)
  }
}

function handleDisconnect(reason: string): void {
  socket = null

  // Reject all pending requests
  for (const [id, request] of pending.entries()) {
    pending.delete(id)
    request.reject(new Error(`Relay disconnected: ${reason}`))
  }

  // Update all attached tabs
  for (const tabId of tabs.getAllTabs().keys()) {
    void chrome.debugger.detach({ tabId }).catch(() => {})
    badge.showDisconnected(tabId)
  }

  tabs.clearAllTabs()
}

export function send(message: RelayMessage): void {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    throw new Error('Relay not connected')
  }
  socket.send(JSON.stringify(message))
}

export function request(message: RelayMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    if (message.id === undefined) {
      reject(new Error('Request requires an ID'))
      return
    }

    pending.set(message.id, { resolve, reject })

    try {
      send(message)
    } catch (err) {
      pending.delete(message.id)
      reject(err instanceof Error ? err : new Error(String(err)))
    }
  })
}

function handleMessage(data: string): void {
  let msg: RelayMessage

  try {
    msg = JSON.parse(data) as RelayMessage
  } catch {
    return
  }

  // Handle ping
  if (msg.method === 'ping') {
    try {
      send({ method: 'pong' })
    } catch {
      // Ignore
    }
    return
  }

  // Handle response to our request
  if (typeof msg.id === 'number' && (msg.result !== undefined || msg.error !== undefined)) {
    const request = pending.get(msg.id)
    if (!request) return

    pending.delete(msg.id)

    if (msg.error) {
      request.reject(new Error(String(msg.error)))
    } else {
      request.resolve(msg.result)
    }
    return
  }

  // Handle forwarded CDP command from relay
  if (typeof msg.id === 'number' && msg.method === 'forwardCDPCommand') {
    void handleCDPCommand(msg)
  }
}

async function handleCDPCommand(msg: RelayMessage): Promise<void> {
  const params = msg.params as Record<string, unknown> | undefined
  const method = String(params?.method || '').trim()
  const cmdParams = params?.params as Record<string, unknown> | undefined
  const sessionId = typeof params?.sessionId === 'string' ? params.sessionId : undefined

  try {
    const result = await executeCDPCommand(method, cmdParams, sessionId)
    send({ id: msg.id, result })
  } catch (err) {
    send({ id: msg.id, error: err instanceof Error ? err.message : String(err) })
  }
}

async function executeCDPCommand(
  method: string,
  params: Record<string, unknown> | undefined,
  sessionId: string | undefined
): Promise<unknown> {
  // Find the target tab
  const bySession = sessionId ? tabs.getTabBySession(sessionId) : null
  const targetId = typeof params?.targetId === 'string' ? params.targetId : undefined

  let tabId = bySession?.tabId
    ?? (targetId ? tabs.getTabByTarget(targetId) : null)
    ?? tabs.getFirstConnectedTab()

  if (!tabId) {
    throw new Error(`No attached tab for ${method}`)
  }

  const debuggee: chrome.debugger.Debuggee = { tabId }

  // Handle special commands
  if (method === 'Runtime.enable') {
    // Reset runtime state first
    try {
      await chrome.debugger.sendCommand(debuggee, 'Runtime.disable')
      await new Promise(r => setTimeout(r, 50))
    } catch {
      // Ignore
    }
    return await chrome.debugger.sendCommand(debuggee, 'Runtime.enable', params)
  }

  if (method === 'Target.createTarget') {
    const url = typeof params?.url === 'string' ? params.url : 'about:blank'
    const newTab = await chrome.tabs.create({ url, active: false })

    if (!newTab.id) {
      throw new Error('Failed to create tab')
    }

    await new Promise(r => setTimeout(r, 100))

    const { sessionId: newSession, targetId: newTarget } = await tabs.attachToTab(newTab.id)
    const port = await getActivePort()

    tabs.setConnected(newTab.id, newSession, newTarget, port)

    // Notify relay of attachment
    send({
      method: 'forwardCDPEvent',
      params: {
        method: 'Target.attachedToTarget',
        params: {
          sessionId: newSession,
          targetInfo: { targetId: newTarget, attached: true },
          waitingForDebugger: false
        }
      }
    })

    return { targetId: newTarget }
  }

  if (method === 'Target.closeTarget') {
    const target = typeof params?.targetId === 'string' ? params.targetId : ''
    const toClose = target ? tabs.getTabByTarget(target) : tabId

    if (!toClose) {
      return { success: false }
    }

    try {
      await chrome.tabs.remove(toClose)
      return { success: true }
    } catch {
      return { success: false }
    }
  }

  if (method === 'Target.activateTarget') {
    const target = typeof params?.targetId === 'string' ? params.targetId : ''
    const toActivate = target ? tabs.getTabByTarget(target) : tabId

    if (!toActivate) return {}

    const tab = await chrome.tabs.get(toActivate).catch(() => null)
    if (!tab) return {}

    if (tab.windowId) {
      await chrome.windows.update(tab.windowId, { focused: true }).catch(() => {})
    }
    await chrome.tabs.update(toActivate, { active: true }).catch(() => {})

    return {}
  }

  // Forward command to debugger
  const tabState = tabs.getTab(tabId)
  const mainSession = tabState?.sessionId

  const debuggerSession = sessionId && mainSession && sessionId !== mainSession
    ? { ...debuggee, sessionId }
    : debuggee

  return await chrome.debugger.sendCommand(debuggerSession, method, params)
}

function handleDebuggerEvent(
  source: chrome.debugger.Debuggee,
  method: string,
  params?: Record<string, unknown>
): void {
  const tabId = source.tabId
  if (!tabId) return

  const tab = tabs.getTab(tabId)
  if (!tab?.sessionId) return

  // Track child sessions
  if (method === 'Target.attachedToTarget' && params?.sessionId) {
    tabs.registerChildSession(String(params.sessionId), tabId)
  }

  if (method === 'Target.detachedFromTarget' && params?.sessionId) {
    tabs.unregisterChildSession(String(params.sessionId))
  }

  // Forward event to relay
  try {
    // source may have sessionId for child targets (not in type but present at runtime)
    const sourceSession = (source as chrome.debugger.Debuggee & { sessionId?: string }).sessionId
    send({
      method: 'forwardCDPEvent',
      params: {
        sessionId: sourceSession || tab.sessionId,
        method,
        params
      }
    })
  } catch {
    // Ignore
  }
}

function handleDebuggerDetach(
  source: chrome.debugger.Debuggee,
  reason: string
): void {
  const tabId = source.tabId
  if (!tabId) return

  const tab = tabs.getTab(tabId)
  if (!tab) return

  // Notify relay of detachment
  if (tab.sessionId && tab.targetId) {
    try {
      send({
        method: 'forwardCDPEvent',
        params: {
          method: 'Target.detachedFromTarget',
          params: {
            sessionId: tab.sessionId,
            targetId: tab.targetId,
            reason
          }
        }
      })
    } catch {
      // Ignore
    }
  }

  tabs.removeTab(tabId)
}

export async function showHelpOnFirstError(): Promise<void> {
  const shown = await hasShownHelp()
  if (shown) return

  await markHelpShown()
  await chrome.runtime.openOptionsPage()
}
