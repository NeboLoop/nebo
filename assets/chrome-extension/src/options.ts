/**
 * Nebo Browser Relay - Options Page
 */

import { DEFAULT_CONFIG, DEFAULT_HOST, RELAY_PATH } from './types.js'
import { loadConfig, setActivePort, getRelayUrl } from './storage.js'

const $ = <T extends HTMLElement>(id: string): T => document.getElementById(id) as T

async function checkRelayStatus(): Promise<boolean> {
  const { http } = await getRelayUrl()
  const ctrl = new AbortController()
  const timeout = setTimeout(() => ctrl.abort(), 1000)

  try {
    const res = await fetch(http, { method: 'HEAD', signal: ctrl.signal })
    return res.ok
  } catch {
    return false
  } finally {
    clearTimeout(timeout)
  }
}

function setStatus(kind: 'ok' | 'error' | '', message: string): void {
  const status = $<HTMLDivElement>('status')
  status.dataset.kind = kind
  status.textContent = message
}

function updateRelayUrl(port: number): void {
  $<HTMLElement>('relay-url').textContent = `http://127.0.0.1:${port}${RELAY_PATH}/`
}

async function load(): Promise<void> {
  const config = await loadConfig()
  const port = config.port

  $<HTMLInputElement>('port').value = String(port)
  updateRelayUrl(port)

  const reachable = await checkRelayStatus()

  if (reachable) {
    setStatus('ok', `Relay reachable`)
  } else {
    setStatus('error', `Relay not reachable. Start Nebo first.`)
  }
}

async function save(): Promise<void> {
  const input = $<HTMLInputElement>('port')
  const raw = Number.parseInt(input.value, 10)
  const port = Number.isFinite(raw) && raw > 0 && raw <= 65535
    ? raw
    : DEFAULT_CONFIG.port

  await setActivePort(port)
  input.value = String(port)
  updateRelayUrl(port)

  const reachable = await checkRelayStatus()

  if (reachable) {
    setStatus('ok', `Saved! Relay reachable`)
  } else {
    setStatus('error', `Saved port ${port}, but relay not reachable. Start Nebo first.`)
  }
}

// Initialize
$<HTMLButtonElement>('save').addEventListener('click', () => void save())
void load()
