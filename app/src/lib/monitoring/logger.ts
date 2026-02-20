/**
 * Application Logging System
 *
 * A structured logging utility that provides consistent log formatting,
 * multiple log levels, and integration points for external logging services.
 */

export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

export interface LogContext {
	/** Unique identifier for tracing requests across services */
	traceId?: string;
	/** User identifier for user-specific logging */
	userId?: string;
	/** Component or module name generating the log */
	component?: string;
	/** HTTP method for API-related logs */
	method?: string;
	/** URL or endpoint for API-related logs */
	url?: string;
	/** HTTP status code for response logs */
	statusCode?: number;
	/** Duration in milliseconds for timing-related logs */
	durationMs?: number;
	/** Any additional context data */
	[key: string]: unknown;
}

export interface LogEntry {
	level: LogLevel;
	message: string;
	timestamp: string;
	context?: LogContext;
	error?: Error;
}

export interface LoggerConfig {
	/** Minimum log level to output */
	minLevel: LogLevel;
	/** Enable console output */
	enableConsole: boolean;
	/** Enable structured JSON output (for log aggregation services) */
	enableJsonOutput: boolean;
	/** Application name for log identification */
	appName: string;
	/** Environment (development, staging, production) */
	environment: string;
}

type LogHandler = (entry: LogEntry) => void;

const LOG_LEVEL_PRIORITY: Record<LogLevel, number> = {
	debug: 0,
	info: 1,
	warn: 2,
	error: 3
};

class Logger {
	private config: LoggerConfig;
	private handlers: LogHandler[] = [];
	private globalContext: LogContext = {};

	constructor(config?: Partial<LoggerConfig>) {
		this.config = {
			minLevel: 'debug',
			enableConsole: true,
			enableJsonOutput: false,
			appName: 'nebo',
			environment: 'development',
			...config
		};
	}

	/**
	 * Update logger configuration
	 */
	configure(config: Partial<LoggerConfig>): void {
		this.config = { ...this.config, ...config };
	}

	/**
	 * Set global context that will be included in all log entries
	 */
	setGlobalContext(context: LogContext): void {
		this.globalContext = { ...this.globalContext, ...context };
	}

	/**
	 * Clear global context
	 */
	clearGlobalContext(): void {
		this.globalContext = {};
	}

	/**
	 * Add a custom log handler for external integrations
	 */
	addHandler(handler: LogHandler): void {
		this.handlers.push(handler);
	}

	/**
	 * Remove a log handler
	 */
	removeHandler(handler: LogHandler): void {
		const index = this.handlers.indexOf(handler);
		if (index > -1) {
			this.handlers.splice(index, 1);
		}
	}

	/**
	 * Create a child logger with pre-set context
	 */
	child(context: LogContext): ChildLogger {
		return new ChildLogger(this, context);
	}

	/**
	 * Log a debug message
	 */
	debug(message: string, context?: LogContext): void {
		this.log('debug', message, context);
	}

	/**
	 * Log an info message
	 */
	info(message: string, context?: LogContext): void {
		this.log('info', message, context);
	}

	/**
	 * Log a warning message
	 */
	warn(message: string, context?: LogContext): void {
		this.log('warn', message, context);
	}

	/**
	 * Log an error message
	 */
	error(message: string, error?: Error | unknown, context?: LogContext): void {
		const errorObj = error instanceof Error ? error : undefined;
		const contextWithError =
			error && !(error instanceof Error) ? { ...context, errorData: error } : context;
		this.log('error', message, contextWithError, errorObj);
	}

	/**
	 * Core logging method
	 */
	private log(level: LogLevel, message: string, context?: LogContext, error?: Error): void {
		if (LOG_LEVEL_PRIORITY[level] < LOG_LEVEL_PRIORITY[this.config.minLevel]) {
			return;
		}

		const entry: LogEntry = {
			level,
			message,
			timestamp: new Date().toISOString(),
			context: { ...this.globalContext, ...context },
			error
		};

		// Console output
		if (this.config.enableConsole) {
			this.outputToConsole(entry);
		}

		// Call registered handlers
		for (const handler of this.handlers) {
			try {
				handler(entry);
			} catch (handlerError) {
				console.error('Log handler error:', handlerError);
			}
		}
	}

	/**
	 * Output log entry to console with appropriate formatting
	 */
	private outputToConsole(entry: LogEntry): void {
		const { level, message, timestamp, context, error } = entry;

		if (this.config.enableJsonOutput) {
			// Structured JSON output for log aggregation
			const jsonLog = {
				timestamp,
				level,
				message,
				app: this.config.appName,
				env: this.config.environment,
				...context,
				...(error && {
					error: {
						name: error.name,
						message: error.message,
						stack: error.stack
					}
				})
			};
			console[level === 'debug' ? 'log' : level](JSON.stringify(jsonLog));
		} else {
			// Human-readable format for development
			const prefix = `[${timestamp}] [${level.toUpperCase()}]`;
			const contextStr =
				context && Object.keys(context).length > 0 ? ` ${JSON.stringify(context)}` : '';

			switch (level) {
				case 'debug':
					console.log(`${prefix} ${message}${contextStr}`);
					break;
				case 'info':
					console.info(`${prefix} ${message}${contextStr}`);
					break;
				case 'warn':
					console.warn(`${prefix} ${message}${contextStr}`);
					break;
				case 'error':
					console.error(`${prefix} ${message}${contextStr}`, error || '');
					break;
			}
		}
	}
}

/**
 * Child logger with pre-set context
 */
class ChildLogger {
	private parent: Logger;
	private context: LogContext;

	constructor(parent: Logger, context: LogContext) {
		this.parent = parent;
		this.context = context;
	}

	debug(message: string, context?: LogContext): void {
		this.parent.debug(message, { ...this.context, ...context });
	}

	info(message: string, context?: LogContext): void {
		this.parent.info(message, { ...this.context, ...context });
	}

	warn(message: string, context?: LogContext): void {
		this.parent.warn(message, { ...this.context, ...context });
	}

	error(message: string, error?: Error | unknown, context?: LogContext): void {
		this.parent.error(message, error, { ...this.context, ...context });
	}
}

// Create and export singleton logger instance
// In production builds, suppress debug output; in dev, show everything
export const logger = new Logger({
	minLevel: import.meta.env.DEV ? 'debug' : 'info'
});

// Export Logger class for testing and custom instances
export { Logger, ChildLogger };
