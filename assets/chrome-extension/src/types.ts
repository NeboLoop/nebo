/**
 * Nebo Browser Relay - Type Definitions
 */

export const DEFAULT_HOST = '127.0.0.1'
export const DEFAULT_PORT = 27895
export const RELAY_PATH = '/relay'

export interface NeboConfig {
  host: string
  port: number
  profiles: ProfileConfig[]
}

export interface ProfileConfig {
  name: string
  port: number
  enabled: boolean
}

export const DEFAULT_CONFIG: NeboConfig = {
  host: DEFAULT_HOST,
  port: DEFAULT_PORT,
  profiles: [
    { name: 'chrome', port: DEFAULT_PORT, enabled: true }
  ]
}

export type BadgeState = 'on' | 'off' | 'connecting' | 'error'

export interface BadgeConfig {
  text: string
  color: string
}

export const BADGE_STATES: Record<BadgeState, BadgeConfig> = {
  on: { text: 'ON', color: '#ffbe18' },
  off: { text: '', color: '#000000' },
  connecting: { text: '…', color: '#F59E0B' },
  error: { text: '!', color: '#B91C1C' }
}

export type TabState = 'connecting' | 'connected'

export interface AttachedTab {
  state: TabState
  sessionId?: string
  targetId?: string
  attachOrder?: number
  port: number
}

export interface RelayMessage {
  id?: number
  method?: string
  params?: Record<string, unknown>
  result?: unknown
  error?: string
}

export interface CDPTargetInfo {
  targetId: string
  type: string
  title: string
  url: string
  attached: boolean
  browserContextId?: string
}

export interface PendingRequest {
  resolve: (value: unknown) => void
  reject: (error: Error) => void
}
