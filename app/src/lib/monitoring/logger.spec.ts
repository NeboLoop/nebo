import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { Logger, type LogContext, type LogEntry } from './logger';

describe('Logger', () => {
	let consoleLogSpy: ReturnType<typeof vi.spyOn>;
	let consoleInfoSpy: ReturnType<typeof vi.spyOn>;
	let consoleWarnSpy: ReturnType<typeof vi.spyOn>;
	let consoleErrorSpy: ReturnType<typeof vi.spyOn>;

	beforeEach(() => {
		consoleLogSpy = vi.spyOn(console, 'log').mockImplementation(() => {});
		consoleInfoSpy = vi.spyOn(console, 'info').mockImplementation(() => {});
		consoleWarnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
		consoleErrorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
	});

	afterEach(() => {
		vi.restoreAllMocks();
	});

	describe('log levels', () => {
		it('should log debug messages when minLevel is debug', () => {
			const logger = new Logger({ minLevel: 'debug', enableConsole: true });
			logger.debug('test message');

			expect(consoleLogSpy).toHaveBeenCalled();
		});

		it('should not log debug messages when minLevel is info', () => {
			const logger = new Logger({ minLevel: 'info', enableConsole: true });
			logger.debug('test message');

			expect(consoleLogSpy).not.toHaveBeenCalled();
		});

		it('should log info messages', () => {
			const logger = new Logger({ minLevel: 'info', enableConsole: true });
			logger.info('test message');

			expect(consoleInfoSpy).toHaveBeenCalled();
		});

		it('should log warn messages', () => {
			const logger = new Logger({ minLevel: 'warn', enableConsole: true });
			logger.warn('test message');

			expect(consoleWarnSpy).toHaveBeenCalled();
		});

		it('should log error messages', () => {
			const logger = new Logger({ minLevel: 'error', enableConsole: true });
			logger.error('test message');

			expect(consoleErrorSpy).toHaveBeenCalled();
		});

		it('should respect log level hierarchy', () => {
			const logger = new Logger({ minLevel: 'warn', enableConsole: true });

			logger.debug('debug');
			logger.info('info');
			logger.warn('warn');
			logger.error('error');

			expect(consoleLogSpy).not.toHaveBeenCalled();
			expect(consoleInfoSpy).not.toHaveBeenCalled();
			expect(consoleWarnSpy).toHaveBeenCalled();
			expect(consoleErrorSpy).toHaveBeenCalled();
		});
	});

	describe('context', () => {
		it('should include context in log output', () => {
			const logger = new Logger({ enableConsole: true, enableJsonOutput: false });
			const context: LogContext = { userId: '123', component: 'TestComponent' };

			logger.info('test message', context);

			expect(consoleInfoSpy).toHaveBeenCalledWith(
				expect.stringContaining('test message'),
				expect.anything()
			);
		});

		it('should merge global context with log context', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });
			logger.addHandler(handler);

			logger.setGlobalContext({ appVersion: '1.0.0' });
			logger.info('test', { userId: '123' });

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					context: expect.objectContaining({
						appVersion: '1.0.0',
						userId: '123'
					})
				})
			);
		});

		it('should clear global context', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });
			logger.addHandler(handler);

			logger.setGlobalContext({ appVersion: '1.0.0' });
			logger.clearGlobalContext();
			logger.info('test', { userId: '123' });

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					context: expect.objectContaining({
						userId: '123'
					})
				})
			);

			// Global context should not be present
			const call = handler.mock.calls[0][0] as LogEntry;
			expect(call.context?.appVersion).toBeUndefined();
		});
	});

	describe('handlers', () => {
		it('should call registered handlers', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });

			logger.addHandler(handler);
			logger.info('test message');

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					level: 'info',
					message: 'test message',
					timestamp: expect.any(String)
				})
			);
		});

		it('should remove handlers', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });

			logger.addHandler(handler);
			logger.removeHandler(handler);
			logger.info('test message');

			expect(handler).not.toHaveBeenCalled();
		});

		it('should handle handler errors gracefully', () => {
			const badHandler = vi.fn().mockImplementation(() => {
				throw new Error('Handler error');
			});
			const goodHandler = vi.fn();
			const logger = new Logger({ enableConsole: false });

			logger.addHandler(badHandler);
			logger.addHandler(goodHandler);

			// Should not throw
			expect(() => logger.info('test')).not.toThrow();

			// Good handler should still be called
			expect(goodHandler).toHaveBeenCalled();
		});
	});

	describe('child logger', () => {
		it('should create child logger with preset context', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });
			logger.addHandler(handler);

			const childLogger = logger.child({ component: 'ChildComponent' });
			childLogger.info('child message');

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					context: expect.objectContaining({
						component: 'ChildComponent'
					})
				})
			);
		});

		it('should merge child context with additional context', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });
			logger.addHandler(handler);

			const childLogger = logger.child({ component: 'ChildComponent' });
			childLogger.info('child message', { action: 'test' });

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					context: expect.objectContaining({
						component: 'ChildComponent',
						action: 'test'
					})
				})
			);
		});
	});

	describe('error logging', () => {
		it('should include error object in log entry', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });
			logger.addHandler(handler);

			const error = new Error('Test error');
			logger.error('error occurred', error);

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					level: 'error',
					error: error
				})
			);
		});

		it('should handle non-Error objects', () => {
			const handler = vi.fn();
			const logger = new Logger({ enableConsole: false });
			logger.addHandler(handler);

			logger.error('error occurred', { code: 'INVALID' });

			expect(handler).toHaveBeenCalledWith(
				expect.objectContaining({
					level: 'error',
					context: expect.objectContaining({
						errorData: { code: 'INVALID' }
					})
				})
			);
		});
	});

	describe('JSON output', () => {
		it('should output JSON when enableJsonOutput is true', () => {
			const logger = new Logger({
				enableConsole: true,
				enableJsonOutput: true,
				appName: 'test-app',
				environment: 'test'
			});

			logger.info('test message', { userId: '123' });

			expect(consoleInfoSpy).toHaveBeenCalled();
			const output = consoleInfoSpy.mock.calls[0][0];
			const parsed = JSON.parse(output);

			expect(parsed).toMatchObject({
				level: 'info',
				message: 'test message',
				app: 'test-app',
				env: 'test',
				userId: '123'
			});
		});
	});

	describe('configuration', () => {
		it('should update configuration', () => {
			const logger = new Logger({ minLevel: 'debug', enableConsole: true });

			logger.debug('should appear');
			expect(consoleLogSpy).toHaveBeenCalled();

			consoleLogSpy.mockClear();
			logger.configure({ minLevel: 'info' });

			logger.debug('should not appear');
			expect(consoleLogSpy).not.toHaveBeenCalled();
		});
	});
});
