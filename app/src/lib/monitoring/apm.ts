/**
 * Application Performance Monitoring (APM)
 *
 * Provides performance tracking, timing measurements, and metrics collection
 * for monitoring application health and identifying bottlenecks.
 */

import { logger } from './logger';

export interface PerformanceMetric {
	name: string;
	value: number;
	unit: 'ms' | 's' | 'bytes' | 'count' | 'percent';
	tags?: Record<string, string>;
	timestamp: string;
}

export interface TransactionContext {
	name: string;
	operation: string;
	data?: Record<string, unknown>;
}

export interface SpanContext {
	name: string;
	operation?: string;
	description?: string;
}

type MetricHandler = (metric: PerformanceMetric) => void;

class APMService {
	private metricHandlers: MetricHandler[] = [];
	private performanceMarks: Map<string, number> = new Map();
	private transactionStack: TransactionContext[] = [];

	constructor() {
		// Initialize performance observer if available
		this.setupPerformanceObserver();
	}

	/**
	 * Setup Performance Observer for Web Vitals
	 */
	private setupPerformanceObserver(): void {
		if (typeof window === 'undefined' || !('PerformanceObserver' in window)) {
			return;
		}

		try {
			// Observe Largest Contentful Paint (LCP)
			const lcpObserver = new PerformanceObserver((list) => {
				const entries = list.getEntries();
				const lastEntry = entries[entries.length - 1];
				if (lastEntry) {
					this.recordMetric('web_vitals.lcp', lastEntry.startTime, 'ms', { vital: 'lcp' });
				}
			});
			lcpObserver.observe({ type: 'largest-contentful-paint', buffered: true });

			// Observe First Input Delay (FID)
			const fidObserver = new PerformanceObserver((list) => {
				const entries = list.getEntries();
				for (const entry of entries) {
					const fidEntry = entry as PerformanceEventTiming;
					this.recordMetric(
						'web_vitals.fid',
						fidEntry.processingStart - fidEntry.startTime,
						'ms',
						{ vital: 'fid' }
					);
				}
			});
			fidObserver.observe({ type: 'first-input', buffered: true });

			// Observe Cumulative Layout Shift (CLS)
			let clsValue = 0;
			const clsObserver = new PerformanceObserver((list) => {
				for (const entry of list.getEntries()) {
					const layoutShiftEntry = entry as LayoutShift;
					if (!layoutShiftEntry.hadRecentInput) {
						clsValue += layoutShiftEntry.value;
					}
				}
				this.recordMetric('web_vitals.cls', clsValue, 'count', { vital: 'cls' });
			});
			clsObserver.observe({ type: 'layout-shift', buffered: true });

			// Observe Time to First Byte (TTFB)
			const navObserver = new PerformanceObserver((list) => {
				const entries = list.getEntries();
				for (const entry of entries) {
					const navEntry = entry as PerformanceNavigationTiming;
					this.recordMetric(
						'web_vitals.ttfb',
						navEntry.responseStart - navEntry.requestStart,
						'ms',
						{ vital: 'ttfb' }
					);
					this.recordMetric(
						'navigation.dom_content_loaded',
						navEntry.domContentLoadedEventEnd - navEntry.domContentLoadedEventStart,
						'ms'
					);
					this.recordMetric(
						'navigation.load',
						navEntry.loadEventEnd - navEntry.loadEventStart,
						'ms'
					);
				}
			});
			navObserver.observe({ type: 'navigation', buffered: true });

			logger.debug('Performance observers initialized');
		} catch (error) {
			logger.warn('Failed to setup performance observers', { error: String(error) });
		}
	}

	/**
	 * Add a handler for recorded metrics
	 */
	addMetricHandler(handler: MetricHandler): void {
		this.metricHandlers.push(handler);
	}

	/**
	 * Remove a metric handler
	 */
	removeMetricHandler(handler: MetricHandler): void {
		const index = this.metricHandlers.indexOf(handler);
		if (index > -1) {
			this.metricHandlers.splice(index, 1);
		}
	}

	/**
	 * Record a performance metric
	 */
	recordMetric(
		name: string,
		value: number,
		unit: PerformanceMetric['unit'] = 'ms',
		tags?: Record<string, string>
	): void {
		const metric: PerformanceMetric = {
			name,
			value,
			unit,
			tags,
			timestamp: new Date().toISOString()
		};

		// Log the metric
		logger.debug('Performance metric recorded', {
			metricName: name,
			value,
			unit,
			...tags
		});

		// Call registered handlers
		for (const handler of this.metricHandlers) {
			try {
				handler(metric);
			} catch (error) {
				logger.error('Metric handler error', error);
			}
		}
	}

	/**
	 * Start a performance mark
	 */
	startMark(name: string): void {
		this.performanceMarks.set(name, performance.now());

		// Use Performance API if available
		if (typeof performance !== 'undefined' && performance.mark) {
			performance.mark(`${name}_start`);
		}
	}

	/**
	 * End a performance mark and record the duration
	 */
	endMark(name: string, tags?: Record<string, string>): number | null {
		const startTime = this.performanceMarks.get(name);
		if (startTime === undefined) {
			logger.warn(`Performance mark "${name}" not found`);
			return null;
		}

		const endTime = performance.now();
		const duration = endTime - startTime;

		// Use Performance API if available
		if (typeof performance !== 'undefined' && performance.mark && performance.measure) {
			performance.mark(`${name}_end`);
			try {
				performance.measure(name, `${name}_start`, `${name}_end`);
			} catch {
				// Measure might fail if marks were cleared
			}
		}

		this.performanceMarks.delete(name);
		this.recordMetric(name, duration, 'ms', tags);

		return duration;
	}

	/**
	 * Measure the duration of an async function
	 */
	async measureAsync<T>(
		name: string,
		fn: () => Promise<T>,
		tags?: Record<string, string>
	): Promise<T> {
		this.startMark(name);
		try {
			const result = await fn();
			this.endMark(name, { ...tags, status: 'success' });
			return result;
		} catch (error) {
			this.endMark(name, { ...tags, status: 'error' });
			throw error;
		}
	}

	/**
	 * Measure the duration of a sync function
	 */
	measure<T>(name: string, fn: () => T, tags?: Record<string, string>): T {
		this.startMark(name);
		try {
			const result = fn();
			this.endMark(name, { ...tags, status: 'success' });
			return result;
		} catch (error) {
			this.endMark(name, { ...tags, status: 'error' });
			throw error;
		}
	}

	/**
	 * Track API request performance
	 */
	trackApiRequest(context: {
		method: string;
		url: string;
		statusCode?: number;
		durationMs: number;
		requestSize?: number;
		responseSize?: number;
	}): void {
		const { method, url, statusCode, durationMs, requestSize, responseSize } = context;

		// Extract endpoint name from URL
		const endpoint = this.extractEndpoint(url);

		this.recordMetric('api.request.duration', durationMs, 'ms', {
			method,
			endpoint,
			status: statusCode?.toString() || 'unknown'
		});

		if (requestSize !== undefined) {
			this.recordMetric('api.request.size', requestSize, 'bytes', { method, endpoint });
		}

		if (responseSize !== undefined) {
			this.recordMetric('api.response.size', responseSize, 'bytes', { method, endpoint });
		}

		// Log slow requests
		if (durationMs > 1000) {
			logger.warn('Slow API request detected', {
				method,
				url,
				durationMs,
				statusCode
			});
		}
	}

	/**
	 * Track component render performance
	 */
	trackComponentRender(componentName: string, durationMs: number): void {
		this.recordMetric('component.render', durationMs, 'ms', {
			component: componentName
		});

		// Warn on slow renders
		if (durationMs > 16) {
			// 60fps = 16.67ms per frame
			logger.debug('Slow component render', {
				component: componentName,
				durationMs
			});
		}
	}

	/**
	 * Track user interactions
	 */
	trackInteraction(name: string, context?: Record<string, string>): void {
		this.recordMetric('interaction', 1, 'count', {
			interaction: name,
			...context
		});
	}

	/**
	 * Start a performance transaction
	 */
	startTransaction<T>(context: TransactionContext, fn: () => T): T {
		this.transactionStack.push(context);

		const startTime = performance.now();
		try {
			return fn();
		} finally {
			const duration = performance.now() - startTime;
			this.recordMetric(`transaction.${context.operation}`, duration, 'ms', {
				name: context.name
			});
			this.transactionStack.pop();
		}
	}

	/**
	 * Get current transaction context
	 */
	getCurrentTransaction(): TransactionContext | undefined {
		return this.transactionStack[this.transactionStack.length - 1];
	}

	/**
	 * Extract a clean endpoint name from URL
	 */
	private extractEndpoint(url: string): string {
		try {
			const urlObj = new URL(url, 'http://localhost');
			// Replace UUIDs and IDs with placeholders
			return urlObj.pathname
				.replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi, ':id')
				.replace(/\/\d+/g, '/:id');
		} catch {
			return url;
		}
	}

	/**
	 * Get memory usage info (if available)
	 */
	getMemoryInfo(): { usedJSHeapSize: number; totalJSHeapSize: number } | null {
		if (typeof performance !== 'undefined' && 'memory' in performance) {
			const memory = (
				performance as Performance & {
					memory?: { usedJSHeapSize: number; totalJSHeapSize: number };
				}
			).memory;
			if (memory) {
				return {
					usedJSHeapSize: memory.usedJSHeapSize,
					totalJSHeapSize: memory.totalJSHeapSize
				};
			}
		}
		return null;
	}

	/**
	 * Report current memory usage
	 */
	reportMemoryUsage(): void {
		const memInfo = this.getMemoryInfo();
		if (memInfo) {
			this.recordMetric('memory.heap.used', memInfo.usedJSHeapSize, 'bytes');
			this.recordMetric('memory.heap.total', memInfo.totalJSHeapSize, 'bytes');
			this.recordMetric(
				'memory.heap.usage',
				(memInfo.usedJSHeapSize / memInfo.totalJSHeapSize) * 100,
				'percent'
			);
		}
	}
}

// Type definitions for Web APIs
interface PerformanceEventTiming extends PerformanceEntry {
	processingStart: number;
}

interface LayoutShift extends PerformanceEntry {
	value: number;
	hadRecentInput: boolean;
}

// Create and export singleton APM service
export const apm = new APMService();

// Export class for testing
export { APMService };
