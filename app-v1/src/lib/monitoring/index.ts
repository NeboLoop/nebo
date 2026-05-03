/**
 * Monitoring Module
 *
 * Central export for all monitoring, logging, and performance monitoring functionality.
 *
 * @example
 * ```typescript
 * import { initMonitoring, logger, apm, alerts } from '$lib/monitoring';
 *
 * // Initialize all monitoring services
 * await initMonitoring({
 *   environment: import.meta.env.MODE,
 *   release: import.meta.env.VITE_APP_VERSION
 * });
 *
 * // Use logger
 * logger.info('Application started');
 *
 * // Track performance
 * apm.trackApiRequest({
 *   method: 'GET',
 *   url: '/api/users',
 *   durationMs: 150,
 *   statusCode: 200
 * });
 *
 * // Trigger alerts
 * if (errorCount > 10) {
 *   alerts.alertCritical('High error rate detected');
 * }
 * ```
 */

// Re-export all modules
export * from './logger';
export * from './apm';
export * from './alerts';
export * from './config';

// Import for initialization
import { logger, type LoggerConfig } from './logger';
import { apm } from './apm';
import { alerts, type AlertConfig } from './alerts';

export interface MonitoringConfig {
	/** Environment name */
	environment?: string;
	/** Application version/release */
	release?: string;
	/** Logging configuration */
	logging?: Partial<LoggerConfig>;
	/** Alert configuration */
	alerts?: Partial<AlertConfig>;
	/** Enable console output in production */
	enableConsoleInProduction?: boolean;
	/** Enable debug mode */
	debug?: boolean;
}

export interface UserContext {
	id: string;
	email?: string;
	username?: string;
	[key: string]: unknown;
}

let isInitialized = false;

/**
 * Initialize all monitoring services
 *
 * Call this early in your application lifecycle, typically in
 * the root layout component or app initialization.
 *
 * @example
 * ```typescript
 * await initMonitoring({
 *   environment: 'production',
 *   release: '1.0.0'
 * });
 * ```
 */
export async function initMonitoring(config: MonitoringConfig = {}): Promise<void> {
	if (isInitialized) {
		logger.warn('Monitoring already initialized');
		return;
	}

	const {
		environment = 'development',
		release,
		logging,
		alerts: alertConfig,
		enableConsoleInProduction = false,
		debug = false
	} = config;

	const isProduction = environment === 'production';

	// Configure logger
	logger.configure({
		minLevel: isProduction && !debug ? 'info' : 'debug',
		enableConsole: !isProduction || enableConsoleInProduction || debug,
		enableJsonOutput: isProduction,
		environment,
		...logging
	});

	logger.info('Initializing monitoring services', { environment, release });

	// Configure alerts
	if (alertConfig) {
		alerts.configure(alertConfig);
	}

	isInitialized = true;
	logger.info('Monitoring services initialized successfully');
}

/**
 * Check if monitoring has been initialized
 */
export function isMonitoringInitialized(): boolean {
	return isInitialized;
}

/**
 * Set the current user for all monitoring services
 */
export function setMonitoringUser(user: UserContext | null): void {
	if (user) {
		logger.setGlobalContext({ userId: user.id });
		logger.debug('Monitoring user set', { userId: user.id });
	} else {
		logger.clearGlobalContext();
		logger.debug('Monitoring user cleared');
	}
}

/**
 * Create a monitored API request wrapper
 *
 * Automatically tracks request performance and handles errors.
 *
 * @example
 * ```typescript
 * const fetchUsers = createMonitoredFetch('/api/users', 'GET');
 * const users = await fetchUsers();
 * ```
 */
export function createMonitoredFetch<T>(
	url: string,
	method: string = 'GET'
): (options?: RequestInit) => Promise<T> {
	return async (options?: RequestInit): Promise<T> => {
		const startTime = performance.now();
		let statusCode: number | undefined;

		try {
			const response = await fetch(url, {
				method,
				...options
			});

			statusCode = response.status;
			const durationMs = performance.now() - startTime;

			apm.trackApiRequest({
				method,
				url,
				statusCode,
				durationMs
			});

			if (!response.ok) {
				const error = new Error(`HTTP ${statusCode}: ${response.statusText}`);
				logger.error('API request failed', error, { method, url, statusCode });
				throw error;
			}

			return response.json();
		} catch (error) {
			const durationMs = performance.now() - startTime;

			apm.trackApiRequest({
				method,
				url,
				statusCode,
				durationMs
			});

			logger.error('API request error', error, { method, url });
			throw error;
		}
	};
}

/**
 * Wrap an async function with performance monitoring
 *
 * @example
 * ```typescript
 * const loadData = withMonitoring('loadData', async () => {
 *   const data = await fetchData();
 *   return processData(data);
 * });
 * ```
 */
export function withMonitoring<T>(
	name: string,
	fn: () => Promise<T>,
	tags?: Record<string, string>
): () => Promise<T> {
	return async () => {
		return apm.measureAsync(name, fn, tags);
	};
}

/**
 * Create an error boundary handler for Svelte components
 *
 * @example
 * ```svelte
 * <script>
 *   import { createComponentErrorHandler } from '$lib/monitoring';
 *
 *   const handleError = createComponentErrorHandler('MyComponent');
 *
 *   // Use in error handling
 *   try {
 *     doSomething();
 *   } catch (error) {
 *     handleError(error);
 *   }
 * </script>
 * ```
 */
export function createComponentErrorHandler(componentName: string) {
	return (error: unknown) => {
		const errorObj = error instanceof Error ? error : new Error(String(error));
		logger.error(`Error in component: ${componentName}`, errorObj, { component: componentName });
	};
}

/**
 * Track a user interaction
 *
 * @example
 * ```typescript
 * trackInteraction('button_click', { buttonId: 'submit-form' });
 * ```
 */
export function trackInteraction(name: string, context?: Record<string, string>): void {
	apm.trackInteraction(name, context);
}

/**
 * Report a custom metric
 *
 * @example
 * ```typescript
 * reportMetric('items_in_cart', 5, 'count', { userId: '123' });
 * ```
 */
export function reportMetric(
	name: string,
	value: number,
	unit: 'ms' | 's' | 'bytes' | 'count' | 'percent' = 'count',
	tags?: Record<string, string>
): void {
	apm.recordMetric(name, value, unit, tags);
}
