// Monitoring module — re-exports logger for use across the app.
// The auth store and WebSocket client import { logger } from '$lib/monitoring'.

export { logger, Logger, ChildLogger } from './logger';
export type { LogLevel, LogContext, LogEntry, LoggerConfig } from './logger';
