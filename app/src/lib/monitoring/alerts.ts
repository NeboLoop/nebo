/**
 * Alert System
 *
 * Provides alerting capabilities for critical issues and anomalies.
 * Supports multiple notification channels and configurable thresholds.
 */

import { logger, type LogContext } from './logger';
import { apm, type PerformanceMetric } from './apm';

export type AlertSeverity = 'low' | 'medium' | 'high' | 'critical';

export interface Alert {
	id: string;
	name: string;
	message: string;
	severity: AlertSeverity;
	timestamp: string;
	context?: Record<string, unknown>;
	acknowledged: boolean;
	resolvedAt?: string;
}

export interface AlertRule {
	id: string;
	name: string;
	description?: string;
	condition: AlertCondition;
	severity: AlertSeverity;
	cooldownMs: number; // Minimum time between alerts
	enabled: boolean;
	notificationChannels: NotificationChannel[];
}

export interface AlertCondition {
	type: 'threshold' | 'rate' | 'anomaly' | 'pattern';
	metric?: string;
	threshold?: number;
	operator?: 'gt' | 'gte' | 'lt' | 'lte' | 'eq' | 'neq';
	windowMs?: number;
	ratePerMinute?: number;
	pattern?: RegExp;
}

export type NotificationChannel = 'console' | 'webhook' | 'custom';

export interface AlertConfig {
	enabled: boolean;
	maxAlertsPerHour: number;
	defaultCooldownMs: number;
	webhookUrl?: string;
	customHandler?: (alert: Alert) => void | Promise<void>;
}

type AlertHandler = (alert: Alert) => void | Promise<void>;

class AlertService {
	private config: AlertConfig;
	private rules: Map<string, AlertRule> = new Map();
	private activeAlerts: Map<string, Alert> = new Map();
	private alertHistory: Alert[] = [];
	private lastAlertTimes: Map<string, number> = new Map();
	private alertsThisHour = 0;
	private hourlyResetTimer: ReturnType<typeof setInterval> | null = null;
	private handlers: AlertHandler[] = [];
	private metricBuffer: Map<string, number[]> = new Map();

	constructor(config?: Partial<AlertConfig>) {
		this.config = {
			enabled: true,
			maxAlertsPerHour: 100,
			defaultCooldownMs: 60000, // 1 minute
			...config
		};

		// Reset hourly counter
		this.hourlyResetTimer = setInterval(() => {
			this.alertsThisHour = 0;
		}, 3600000);

		// Register default rules
		this.registerDefaultRules();

		// Listen for performance metrics
		apm.addMetricHandler((metric) => this.handleMetric(metric));
	}

	/**
	 * Update alert configuration
	 */
	configure(config: Partial<AlertConfig>): void {
		this.config = { ...this.config, ...config };
	}

	/**
	 * Register a custom alert handler
	 */
	addHandler(handler: AlertHandler): void {
		this.handlers.push(handler);
	}

	/**
	 * Remove an alert handler
	 */
	removeHandler(handler: AlertHandler): void {
		const index = this.handlers.indexOf(handler);
		if (index > -1) {
			this.handlers.splice(index, 1);
		}
	}

	/**
	 * Register default monitoring rules
	 */
	private registerDefaultRules(): void {
		// High API latency
		this.addRule({
			id: 'high-api-latency',
			name: 'High API Latency',
			description: 'API requests taking longer than 3 seconds',
			condition: {
				type: 'threshold',
				metric: 'api.request.duration',
				threshold: 3000,
				operator: 'gt'
			},
			severity: 'high',
			cooldownMs: 60000,
			enabled: true,
			notificationChannels: ['console']
		});

		// Very slow API (critical)
		this.addRule({
			id: 'critical-api-latency',
			name: 'Critical API Latency',
			description: 'API requests taking longer than 10 seconds',
			condition: {
				type: 'threshold',
				metric: 'api.request.duration',
				threshold: 10000,
				operator: 'gt'
			},
			severity: 'critical',
			cooldownMs: 30000,
			enabled: true,
			notificationChannels: ['console']
		});

		// Poor LCP (Largest Contentful Paint)
		this.addRule({
			id: 'poor-lcp',
			name: 'Poor Largest Contentful Paint',
			description: 'LCP exceeds 4 seconds (poor user experience)',
			condition: {
				type: 'threshold',
				metric: 'web_vitals.lcp',
				threshold: 4000,
				operator: 'gt'
			},
			severity: 'medium',
			cooldownMs: 300000, // 5 minutes
			enabled: true,
			notificationChannels: ['console']
		});

		// High memory usage
		this.addRule({
			id: 'high-memory-usage',
			name: 'High Memory Usage',
			description: 'Heap memory usage exceeds 90%',
			condition: {
				type: 'threshold',
				metric: 'memory.heap.usage',
				threshold: 90,
				operator: 'gt'
			},
			severity: 'high',
			cooldownMs: 120000, // 2 minutes
			enabled: true,
			notificationChannels: ['console']
		});

		// High error rate
		this.addRule({
			id: 'high-error-rate',
			name: 'High Error Rate',
			description: 'More than 10 errors per minute',
			condition: {
				type: 'rate',
				metric: 'error',
				ratePerMinute: 10,
				windowMs: 60000
			},
			severity: 'critical',
			cooldownMs: 60000,
			enabled: true,
			notificationChannels: ['console']
		});
	}

	/**
	 * Add or update an alert rule
	 */
	addRule(rule: AlertRule): void {
		this.rules.set(rule.id, rule);
		logger.debug('Alert rule registered', { ruleId: rule.id, ruleName: rule.name });
	}

	/**
	 * Remove an alert rule
	 */
	removeRule(ruleId: string): boolean {
		const removed = this.rules.delete(ruleId);
		if (removed) {
			logger.debug('Alert rule removed', { ruleId });
		}
		return removed;
	}

	/**
	 * Enable or disable a rule
	 */
	setRuleEnabled(ruleId: string, enabled: boolean): void {
		const rule = this.rules.get(ruleId);
		if (rule) {
			rule.enabled = enabled;
			logger.debug('Alert rule status changed', { ruleId, enabled });
		}
	}

	/**
	 * Handle incoming performance metrics
	 */
	private handleMetric(metric: PerformanceMetric): void {
		// Buffer metrics for rate calculations
		const buffer = this.metricBuffer.get(metric.name) || [];
		buffer.push(Date.now());
		// Keep only last minute of data
		const cutoff = Date.now() - 60000;
		const filtered = buffer.filter((t) => t > cutoff);
		this.metricBuffer.set(metric.name, filtered);

		// Check all rules
		for (const rule of this.rules.values()) {
			if (!rule.enabled) continue;
			if (rule.condition.metric !== metric.name) continue;

			if (this.evaluateCondition(rule.condition, metric.value)) {
				this.triggerAlert(rule, {
					metricName: metric.name,
					metricValue: metric.value,
					metricUnit: metric.unit,
					...metric.tags
				});
			}
		}
	}

	/**
	 * Evaluate an alert condition
	 */
	private evaluateCondition(condition: AlertCondition, value: number): boolean {
		switch (condition.type) {
			case 'threshold': {
				const threshold = condition.threshold || 0;
				switch (condition.operator) {
					case 'gt':
						return value > threshold;
					case 'gte':
						return value >= threshold;
					case 'lt':
						return value < threshold;
					case 'lte':
						return value <= threshold;
					case 'eq':
						return value === threshold;
					case 'neq':
						return value !== threshold;
					default:
						return false;
				}
			}
			case 'rate': {
				const buffer = this.metricBuffer.get(condition.metric || '');
				if (!buffer) return false;
				const count = buffer.length;
				return count >= (condition.ratePerMinute || 0);
			}
			default:
				return false;
		}
	}

	/**
	 * Trigger an alert
	 */
	triggerAlert(rule: AlertRule, context?: Record<string, unknown>): Alert | null {
		if (!this.config.enabled) return null;

		// Check cooldown
		const lastAlertTime = this.lastAlertTimes.get(rule.id) || 0;
		if (Date.now() - lastAlertTime < rule.cooldownMs) {
			logger.debug('Alert suppressed due to cooldown', { ruleId: rule.id });
			return null;
		}

		// Check hourly limit
		if (this.alertsThisHour >= this.config.maxAlertsPerHour) {
			logger.warn('Hourly alert limit reached, suppressing alert', {
				ruleId: rule.id,
				limit: this.config.maxAlertsPerHour
			});
			return null;
		}

		const alert: Alert = {
			id: `${rule.id}-${Date.now()}`,
			name: rule.name,
			message: rule.description || rule.name,
			severity: rule.severity,
			timestamp: new Date().toISOString(),
			context,
			acknowledged: false
		};

		// Update tracking
		this.lastAlertTimes.set(rule.id, Date.now());
		this.alertsThisHour++;
		this.activeAlerts.set(alert.id, alert);
		this.alertHistory.push(alert);

		// Keep history manageable
		if (this.alertHistory.length > 1000) {
			this.alertHistory = this.alertHistory.slice(-500);
		}

		// Send notifications
		this.sendNotifications(alert, rule.notificationChannels);

		return alert;
	}

	/**
	 * Send alert notifications to configured channels
	 */
	private async sendNotifications(alert: Alert, channels: NotificationChannel[]): Promise<void> {
		for (const channel of channels) {
			try {
				await this.sendToChannel(alert, channel);
			} catch (error) {
				logger.error(`Failed to send alert to ${channel}`, error, { alertId: alert.id });
			}
		}

		// Call registered handlers
		for (const handler of this.handlers) {
			try {
				await handler(alert);
			} catch (error) {
				logger.error('Alert handler error', error, { alertId: alert.id });
			}
		}
	}

	/**
	 * Send alert to a specific channel
	 */
	private async sendToChannel(alert: Alert, channel: NotificationChannel): Promise<void> {
		switch (channel) {
			case 'console':
				this.logAlert(alert);
				break;

			case 'webhook':
				if (this.config.webhookUrl) {
					await this.sendWebhook(alert);
				}
				break;

			case 'custom':
				if (this.config.customHandler) {
					await this.config.customHandler(alert);
				}
				break;
		}
	}

	/**
	 * Log alert to console with appropriate level
	 */
	private logAlert(alert: Alert): void {
		const context: LogContext = {
			alertId: alert.id,
			severity: alert.severity,
			...alert.context
		};

		switch (alert.severity) {
			case 'critical':
			case 'high':
				logger.error(`[ALERT] ${alert.name}: ${alert.message}`, undefined, context);
				break;
			case 'medium':
				logger.warn(`[ALERT] ${alert.name}: ${alert.message}`, context);
				break;
			case 'low':
				logger.info(`[ALERT] ${alert.name}: ${alert.message}`, context);
				break;
		}
	}

	/**
	 * Send alert to webhook
	 */
	private async sendWebhook(alert: Alert): Promise<void> {
		if (!this.config.webhookUrl) return;

		const payload = {
			alert: {
				id: alert.id,
				name: alert.name,
				message: alert.message,
				severity: alert.severity,
				timestamp: alert.timestamp,
				context: alert.context
			},
			application: 'nebo'
		};

		await fetch(this.config.webhookUrl, {
			method: 'POST',
			headers: {
				'Content-Type': 'application/json'
			},
			body: JSON.stringify(payload)
		});
	}

	/**
	 * Acknowledge an alert
	 */
	acknowledgeAlert(alertId: string): boolean {
		const alert = this.activeAlerts.get(alertId);
		if (alert) {
			alert.acknowledged = true;
			logger.info('Alert acknowledged', { alertId });
			return true;
		}
		return false;
	}

	/**
	 * Resolve an alert
	 */
	resolveAlert(alertId: string): boolean {
		const alert = this.activeAlerts.get(alertId);
		if (alert) {
			alert.resolvedAt = new Date().toISOString();
			this.activeAlerts.delete(alertId);
			logger.info('Alert resolved', { alertId });
			return true;
		}
		return false;
	}

	/**
	 * Get all active alerts
	 */
	getActiveAlerts(): Alert[] {
		return Array.from(this.activeAlerts.values());
	}

	/**
	 * Get alert history
	 */
	getAlertHistory(limit?: number): Alert[] {
		const history = [...this.alertHistory].reverse();
		return limit ? history.slice(0, limit) : history;
	}

	/**
	 * Get all registered rules
	 */
	getRules(): AlertRule[] {
		return Array.from(this.rules.values());
	}

	/**
	 * Manually trigger an error alert
	 */
	alertError(message: string, context?: Record<string, unknown>): Alert | null {
		const rule: AlertRule = {
			id: 'manual-error',
			name: 'Manual Error Alert',
			description: message,
			condition: { type: 'threshold' },
			severity: 'high',
			cooldownMs: 0,
			enabled: true,
			notificationChannels: ['console']
		};
		return this.triggerAlert(rule, context);
	}

	/**
	 * Manually trigger a critical alert
	 */
	alertCritical(message: string, context?: Record<string, unknown>): Alert | null {
		const rule: AlertRule = {
			id: 'manual-critical',
			name: 'Manual Critical Alert',
			description: message,
			condition: { type: 'threshold' },
			severity: 'critical',
			cooldownMs: 0,
			enabled: true,
			notificationChannels: ['console']
		};
		return this.triggerAlert(rule, context);
	}

	/**
	 * Cleanup resources
	 */
	destroy(): void {
		if (this.hourlyResetTimer) {
			clearInterval(this.hourlyResetTimer);
			this.hourlyResetTimer = null;
		}
	}
}

// Create and export singleton alert service
export const alerts = new AlertService();

// Export class for testing
export { AlertService };
