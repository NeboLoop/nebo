/**
 * Nebo Browser Relay - Storage Management
 */

import { DEFAULT_CONFIG, DEFAULT_HOST, DEFAULT_PORT, RELAY_PATH, type NeboConfig, type ProfileConfig } from './types.js'

const STORAGE_KEY = 'neboConfig'

// Old ports that should be migrated to the new default
const LEGACY_PORTS = [9224, 9223, 9222]

// Old hosts that should be migrated to 127.0.0.1 for security
const LEGACY_HOSTS = ['local.nebo.bot', 'localhost']

export async function loadConfig(): Promise<NeboConfig> {
  const stored = await chrome.storage.local.get([STORAGE_KEY])
  if (!stored[STORAGE_KEY]) {
    return { ...DEFAULT_CONFIG }
  }

  const config = stored[STORAGE_KEY] as Partial<NeboConfig>
  let port = validatePort(config.port) ?? DEFAULT_PORT
  let host = config.host ?? DEFAULT_HOST
  let needsMigration = false

  // Migrate legacy ports to new default
  if (LEGACY_PORTS.includes(port)) {
    port = DEFAULT_PORT
    needsMigration = true
  }

  // Migrate legacy hosts to 127.0.0.1 for security
  if (LEGACY_HOSTS.includes(host)) {
    host = DEFAULT_HOST
    needsMigration = true
  }

  if (needsMigration) {
    const migratedConfig: NeboConfig = {
      host: host,
      port: port,
      profiles: config.profiles ?? DEFAULT_CONFIG.profiles
    }
    await saveConfig(migratedConfig)
  }

  return {
    host: host,
    port: port,
    profiles: config.profiles ?? DEFAULT_CONFIG.profiles
  }
}

export async function getRelayUrl(): Promise<{ http: string; ws: string }> {
  const config = await loadConfig()
  return {
    http: `http://${config.host}:${config.port}${RELAY_PATH}`,
    ws: `ws://${config.host}:${config.port}${RELAY_PATH}/extension`
  }
}

export async function saveConfig(config: NeboConfig): Promise<void> {
  await chrome.storage.local.set({ [STORAGE_KEY]: config })
}

export async function getActivePort(): Promise<number> {
  const config = await loadConfig()
  return config.port
}

export async function setActivePort(port: number): Promise<void> {
  const config = await loadConfig()
  config.port = validatePort(port) ?? DEFAULT_CONFIG.port
  await saveConfig(config)
}

export async function getProfiles(): Promise<ProfileConfig[]> {
  const config = await loadConfig()
  return config.profiles
}

export async function addProfile(profile: ProfileConfig): Promise<void> {
  const config = await loadConfig()
  const existing = config.profiles.findIndex(p => p.name === profile.name)
  if (existing >= 0) {
    config.profiles[existing] = profile
  } else {
    config.profiles.push(profile)
  }
  await saveConfig(config)
}

export async function removeProfile(name: string): Promise<void> {
  const config = await loadConfig()
  config.profiles = config.profiles.filter(p => p.name !== name)
  await saveConfig(config)
}

function validatePort(port: unknown): number | null {
  const n = Number.parseInt(String(port ?? ''), 10)
  if (!Number.isFinite(n) || n <= 0 || n > 65535) {
    return null
  }
  return n
}

// Track first-run state
export async function hasShownHelp(): Promise<boolean> {
  const stored = await chrome.storage.local.get(['helpShown'])
  return stored.helpShown === true
}

export async function markHelpShown(): Promise<void> {
  await chrome.storage.local.set({ helpShown: true })
}
