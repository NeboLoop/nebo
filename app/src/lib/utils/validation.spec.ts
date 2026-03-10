import { describe, expect, it } from 'vitest';
import {
	validateUrl,
	validateMarketingCopy,
	sanitizeInput,
	normalizeUrl
} from './validation';

describe('validateUrl', () => {
	describe('valid URLs', () => {
		it('should accept https URLs', () => {
			const result = validateUrl('https://example.com');
			expect(result.isValid).toBe(true);
			expect(result.error).toBeUndefined();
		});

		it('should accept http URLs', () => {
			const result = validateUrl('http://example.com');
			expect(result.isValid).toBe(true);
		});

		it('should accept URLs with paths', () => {
			const result = validateUrl('https://example.com/path/to/page');
			expect(result.isValid).toBe(true);
		});

		it('should accept URLs with query parameters', () => {
			const result = validateUrl('https://example.com?query=value&other=test');
			expect(result.isValid).toBe(true);
		});

		it('should accept URLs with subdomains', () => {
			const result = validateUrl('https://www.subdomain.example.com');
			expect(result.isValid).toBe(true);
		});

		it('should accept URLs with ports', () => {
			const result = validateUrl('https://example.com:8080');
			expect(result.isValid).toBe(true);
		});

		it('should accept localhost URLs', () => {
			const result = validateUrl('http://localhost:3000');
			expect(result.isValid).toBe(true);
		});

		it('should trim whitespace from URLs', () => {
			const result = validateUrl('  https://example.com  ');
			expect(result.isValid).toBe(true);
		});
	});

	describe('invalid URLs', () => {
		it('should reject empty strings', () => {
			const result = validateUrl('');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a URL');
		});

		it('should reject whitespace-only strings', () => {
			const result = validateUrl('   ');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a URL');
		});

		it('should reject URLs without protocol', () => {
			const result = validateUrl('example.com');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('http');
		});

		it('should reject ftp protocol', () => {
			const result = validateUrl('ftp://example.com');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('http');
		});

		it('should reject mailto protocol', () => {
			const result = validateUrl('mailto:test@example.com');
			expect(result.isValid).toBe(false);
		});

		it('should reject URLs without TLD (except localhost)', () => {
			const result = validateUrl('https://example');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('valid domain');
		});

		it('should reject URLs with invalid domain format', () => {
			const result = validateUrl('https://.example.com');
			expect(result.isValid).toBe(false);
		});

		it('should reject malformed URLs', () => {
			const result = validateUrl('https://');
			expect(result.isValid).toBe(false);
		});

		it('should reject random text', () => {
			const result = validateUrl('not a url at all');
			expect(result.isValid).toBe(false);
		});
	});
});

describe('validateMarketingCopy', () => {
	describe('valid marketing copy', () => {
		it('should accept text with sufficient length', () => {
			const result = validateMarketingCopy('This is valid marketing copy.');
			expect(result.isValid).toBe(true);
			expect(result.error).toBeUndefined();
		});

		it('should accept text at minimum length', () => {
			const result = validateMarketingCopy('Valid text'); // exactly 10 chars
			expect(result.isValid).toBe(true);
		});

		it('should accept long marketing copy', () => {
			const longText = 'A'.repeat(5000) + ' marketing copy';
			const result = validateMarketingCopy(longText);
			expect(result.isValid).toBe(true);
		});

		it('should accept text with special characters', () => {
			const result = validateMarketingCopy('Marketing copy with $pecial ch@racters! & symbols.');
			expect(result.isValid).toBe(true);
		});

		it('should accept text with numbers', () => {
			const result = validateMarketingCopy('Get 50% off your first 3 orders today!');
			expect(result.isValid).toBe(true);
		});

		it('should trim and validate correctly', () => {
			const result = validateMarketingCopy('  Valid marketing copy  ');
			expect(result.isValid).toBe(true);
		});
	});

	describe('invalid marketing copy', () => {
		it('should reject empty strings', () => {
			const result = validateMarketingCopy('');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your marketing copy');
		});

		it('should reject whitespace-only strings', () => {
			const result = validateMarketingCopy('   \n\t  ');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your marketing copy');
		});

		it('should reject text below minimum length', () => {
			const result = validateMarketingCopy('Short');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('at least');
		});

		it('should reject text exceeding maximum length', () => {
			const longText = 'A'.repeat(10001);
			const result = validateMarketingCopy(longText);
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('10,000');
		});

		it('should reject text with only numbers', () => {
			const result = validateMarketingCopy('12345678901234567890');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('readable text');
		});

		it('should reject text with only symbols', () => {
			const result = validateMarketingCopy('!@#$%^&*()_+=-{}[]');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('readable text');
		});

		it('should reject text without meaningful words', () => {
			const result = validateMarketingCopy('a b c d e f g h i j');
			expect(result.isValid).toBe(false);
			expect(result.error).toContain('meaningful');
		});
	});
});

describe('sanitizeInput', () => {
	it('should escape HTML tags', () => {
		const result = sanitizeInput('<script>alert("xss")</script>');
		expect(result).toBe('&lt;script&gt;alert(&quot;xss&quot;)&lt;&#x2F;script&gt;');
	});

	it('should escape single quotes', () => {
		const result = sanitizeInput("It's a test");
		expect(result).toBe('It&#x27;s a test');
	});

	it('should escape double quotes', () => {
		const result = sanitizeInput('Say "hello"');
		expect(result).toBe('Say &quot;hello&quot;');
	});

	it('should escape forward slashes', () => {
		const result = sanitizeInput('path/to/file');
		expect(result).toBe('path&#x2F;to&#x2F;file');
	});

	it('should handle empty string', () => {
		const result = sanitizeInput('');
		expect(result).toBe('');
	});

	it('should not modify safe text', () => {
		const result = sanitizeInput('Safe marketing text without special chars');
		expect(result).toBe('Safe marketing text without special chars');
	});
});

describe('normalizeUrl', () => {
	it('should add https:// if no protocol is present', () => {
		const result = normalizeUrl('example.com');
		expect(result).toBe('https://example.com');
	});

	it('should preserve existing https://', () => {
		const result = normalizeUrl('https://example.com');
		expect(result).toBe('https://example.com');
	});

	it('should preserve existing http://', () => {
		const result = normalizeUrl('http://example.com');
		expect(result).toBe('http://example.com');
	});

	it('should trim whitespace', () => {
		const result = normalizeUrl('  example.com  ');
		expect(result).toBe('https://example.com');
	});

	it('should handle URLs with paths', () => {
		const result = normalizeUrl('example.com/path');
		expect(result).toBe('https://example.com/path');
	});
});
