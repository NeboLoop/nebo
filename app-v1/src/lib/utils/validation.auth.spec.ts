import { describe, expect, it } from 'vitest';
import {
	validateEmail,
	validatePassword,
	validatePasswordConfirmation,
	validateName,
	validateLoginForm,
	validateRegistrationForm
} from './validation';

describe('Authentication Validation', () => {
	describe('validateEmail', () => {
		it('should return invalid for empty email', () => {
			const result = validateEmail('');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your email address');
		});

		it('should return invalid for whitespace-only email', () => {
			const result = validateEmail('   ');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your email address');
		});

		it('should return invalid for email without @', () => {
			const result = validateEmail('test.example.com');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a valid email address');
		});

		it('should return invalid for email without domain', () => {
			const result = validateEmail('test@');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a valid email address');
		});

		it('should return invalid for email without TLD', () => {
			const result = validateEmail('test@example');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a valid email address');
		});

		it('should return valid for correct email format', () => {
			const result = validateEmail('test@example.com');
			expect(result.isValid).toBe(true);
			expect(result.error).toBeUndefined();
		});

		it('should return valid for email with subdomain', () => {
			const result = validateEmail('test@mail.example.com');
			expect(result.isValid).toBe(true);
		});

		it('should return valid for email with plus sign', () => {
			const result = validateEmail('test+alias@example.com');
			expect(result.isValid).toBe(true);
		});

		it('should trim whitespace from email', () => {
			const result = validateEmail('  test@example.com  ');
			expect(result.isValid).toBe(true);
		});
	});

	describe('validatePassword', () => {
		it('should return invalid for empty password', () => {
			const result = validatePassword('');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a password');
		});

		it('should return invalid for password shorter than 8 characters', () => {
			const result = validatePassword('Short1!');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Password must be at least 8 characters long');
			expect(result.requirements?.minLength).toBe(false);
		});

		it('should return invalid for password without uppercase', () => {
			const result = validatePassword('password123');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Password must contain at least one uppercase letter');
			expect(result.requirements?.hasUppercase).toBe(false);
		});

		it('should return invalid for password without lowercase', () => {
			const result = validatePassword('PASSWORD123');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Password must contain at least one lowercase letter');
			expect(result.requirements?.hasLowercase).toBe(false);
		});

		it('should return invalid for password without number', () => {
			const result = validatePassword('PasswordABC');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Password must contain at least one number');
			expect(result.requirements?.hasNumber).toBe(false);
		});

		it('should return valid for password meeting all requirements', () => {
			const result = validatePassword('Password123');
			expect(result.isValid).toBe(true);
			expect(result.requirements?.minLength).toBe(true);
			expect(result.requirements?.hasUppercase).toBe(true);
			expect(result.requirements?.hasLowercase).toBe(true);
			expect(result.requirements?.hasNumber).toBe(true);
		});

		it('should track special character requirement', () => {
			const resultWithoutSpecial = validatePassword('Password123');
			expect(resultWithoutSpecial.requirements?.hasSpecialChar).toBe(false);

			const resultWithSpecial = validatePassword('Password123!');
			expect(resultWithSpecial.requirements?.hasSpecialChar).toBe(true);
		});

		it('should return invalid for password exceeding max length', () => {
			const longPassword = 'Password1' + 'a'.repeat(120);
			const result = validatePassword(longPassword);
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Password is too long (maximum 128 characters)');
		});
	});

	describe('validatePasswordConfirmation', () => {
		it('should return invalid for empty confirmation', () => {
			const result = validatePasswordConfirmation('Password123', '');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please confirm your password');
		});

		it('should return invalid for non-matching passwords', () => {
			const result = validatePasswordConfirmation('Password123', 'Password456');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Passwords do not match');
		});

		it('should return valid for matching passwords', () => {
			const result = validatePasswordConfirmation('Password123', 'Password123');
			expect(result.isValid).toBe(true);
		});
	});

	describe('validateName', () => {
		it('should return invalid for empty name', () => {
			const result = validateName('');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your name');
		});

		it('should return invalid for whitespace-only name', () => {
			const result = validateName('   ');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your name');
		});

		it('should return invalid for name shorter than 2 characters', () => {
			const result = validateName('A');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Name must be at least 2 characters');
		});

		it('should return valid for valid name', () => {
			const result = validateName('John Doe');
			expect(result.isValid).toBe(true);
		});

		it('should return valid for name with hyphen', () => {
			const result = validateName('Mary-Jane');
			expect(result.isValid).toBe(true);
		});

		it('should return valid for name with apostrophe', () => {
			const result = validateName("O'Connor");
			expect(result.isValid).toBe(true);
		});

		it('should return valid for name with international characters', () => {
			const result = validateName('Jose Garcia');
			expect(result.isValid).toBe(true);
		});

		it('should return invalid for name with numbers', () => {
			const result = validateName('John123');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Name contains invalid characters');
		});

		it('should return invalid for name exceeding max length', () => {
			const longName = 'A'.repeat(101);
			const result = validateName(longName);
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Name must not exceed 100 characters');
		});
	});

	describe('validateLoginForm', () => {
		it('should return invalid for empty email', () => {
			const result = validateLoginForm('', 'password');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your email address');
		});

		it('should return invalid for invalid email', () => {
			const result = validateLoginForm('invalid-email', 'password');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a valid email address');
		});

		it('should return invalid for empty password', () => {
			const result = validateLoginForm('test@example.com', '');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your password');
		});

		it('should return valid for valid credentials', () => {
			const result = validateLoginForm('test@example.com', 'password');
			expect(result.isValid).toBe(true);
		});
	});

	describe('validateRegistrationForm', () => {
		it('should return invalid for empty name', () => {
			const result = validateRegistrationForm('test@example.com', 'Password123', 'Password123', '');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter your name');
		});

		it('should return invalid for invalid email', () => {
			const result = validateRegistrationForm('invalid', 'Password123', 'Password123', 'John');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Please enter a valid email address');
		});

		it('should return invalid for weak password', () => {
			const result = validateRegistrationForm('test@example.com', 'weak', 'weak', 'John');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Password must be at least 8 characters long');
		});

		it('should return invalid for mismatched passwords', () => {
			const result = validateRegistrationForm('test@example.com', 'Password123', 'Password456', 'John');
			expect(result.isValid).toBe(false);
			expect(result.error).toBe('Passwords do not match');
		});

		it('should return valid for complete valid form', () => {
			const result = validateRegistrationForm('test@example.com', 'Password123', 'Password123', 'John Doe');
			expect(result.isValid).toBe(true);
		});
	});
});
