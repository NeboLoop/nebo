/**
 * Monitoring Configuration
 *
 * Centralized configuration for all monitoring services.
 * Reads from environment variables and provides sensible defaults.
 */

import type { MonitoringConfig } from './index';

/**
 * Get monitoring configuration from environment variables
 *
 * Environment variables (set in .env files):
 * - VITE_APP_VERSION: Application version for release tracking
 * - VITE_ENVIRONMENT: Deployment environment (development, staging, production)
 * - VITE_ENABLE_DEBUG: Enable debug logging
 * - VITE_ALERT_WEBHOOK_URL: Webhook URL for alert notifications
 */
export function getMonitoringConfig(): MonitoringConfig {
	// Check if we're in a browser environment with Vite
	const hasEnv = typeof import.meta !== 'undefined' && import.meta.env;
	const env = hasEnv ? import.meta.env : null;

	const isDevelopment = env?.DEV ?? true;
	const isProduction = env?.PROD ?? false;

	return {
		// Environment detection
		environment: env?.VITE_ENVIRONMENT || (isProduction ? 'production' : 'development'),

		// Release/version tracking
		release: env?.VITE_APP_VERSION || undefined,

		// Debug mode
		debug: env?.VITE_ENABLE_DEBUG === 'true' || isDevelopment,

		// Console output in production (disabled by default for perf)
		enableConsoleInProduction: env?.VITE_ENABLE_CONSOLE_IN_PRODUCTION === 'true',

		// Logging configuration
		logging: {
			minLevel: isDevelopment ? 'debug' : 'info',
			enableConsole: true,
			enableJsonOutput: isProduction,
			appName: 'nebo'
		},

		// Alert configuration
		alerts: {
			enabled: true,
			maxAlertsPerHour: isProduction ? 100 : 1000,
			defaultCooldownMs: isProduction ? 60000 : 5000,
			webhookUrl: env?.VITE_ALERT_WEBHOOK_URL
		}
	};
}

/**
 * Example .env file content for reference:
 *
 * ```env
 * # Application Version (usually set during build)
 * VITE_APP_VERSION=1.0.0
 *
 * # Environment
 * VITE_ENVIRONMENT=development
 *
 * # Enable debug mode
 * VITE_ENABLE_DEBUG=true
 *
 * # Alert webhook (optional)
 * VITE_ALERT_WEBHOOK_URL=https://hooks.slack.com/services/xxx/yyy/zzz
 * ```
 */
export const ENV_EXAMPLE = `
# Monitoring Configuration
# Copy this to .env.local and fill in your values

# Application Version (set during CI/CD build)
VITE_APP_VERSION=

# Environment (development, staging, production)
VITE_ENVIRONMENT=development

# Enable debug mode (shows debug logs)
VITE_ENABLE_DEBUG=false

# Enable console output in production
VITE_ENABLE_CONSOLE_IN_PRODUCTION=false

# Alert webhook URL (optional, for Slack/Discord/etc notifications)
VITE_ALERT_WEBHOOK_URL=
`;
