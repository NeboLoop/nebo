export interface ValidationResult {
	isValid: boolean;
	error?: string;
}

/**
 * Validates a URL string
 * - Must be a valid URL format
 * - Must use http or https protocol
 * - Must have a valid domain structure
 */
export function validateUrl(url: string): ValidationResult {
	if (!url || url.trim() === '') {
		return {
			isValid: false,
			error: 'Please enter a URL'
		};
	}

	const trimmedUrl = url.trim();

	// Check for valid URL format
	try {
		const parsedUrl = new URL(trimmedUrl);

		// Must be http or https
		if (!['http:', 'https:'].includes(parsedUrl.protocol)) {
			return {
				isValid: false,
				error: 'URL must start with http:// or https://'
			};
		}

		// Must have a valid hostname
		if (!parsedUrl.hostname || parsedUrl.hostname.length < 1) {
			return {
				isValid: false,
				error: 'Please enter a valid domain'
			};
		}

		// Basic domain validation - must have at least one dot for TLD (unless localhost)
		if (!parsedUrl.hostname.includes('.') && parsedUrl.hostname !== 'localhost') {
			return {
				isValid: false,
				error: 'Please enter a complete URL with a valid domain'
			};
		}

		// Check for common invalid patterns
		if (parsedUrl.hostname.startsWith('.') || parsedUrl.hostname.endsWith('.')) {
			return {
				isValid: false,
				error: 'Invalid domain format'
			};
		}

		return { isValid: true };
	} catch {
		// If URL parsing fails, provide helpful message
		if (!trimmedUrl.startsWith('http://') && !trimmedUrl.startsWith('https://')) {
			return {
				isValid: false,
				error: 'URL must start with http:// or https://'
			};
		}

		return {
			isValid: false,
			error: 'Please enter a valid URL'
		};
	}
}

/**
 * Validates marketing copy text
 * - Must not be empty
 * - Must have minimum length (10 characters)
 * - Must not exceed maximum length (10,000 characters)
 * - Must contain some meaningful content
 */
export function validateMarketingCopy(text: string): ValidationResult {
	if (!text || text.trim() === '') {
		return {
			isValid: false,
			error: 'Please enter your marketing copy'
		};
	}

	const trimmedText = text.trim();
	const MIN_LENGTH = 10;
	const MAX_LENGTH = 10000;

	if (trimmedText.length < MIN_LENGTH) {
		return {
			isValid: false,
			error: `Marketing copy must be at least ${MIN_LENGTH} characters`
		};
	}

	if (trimmedText.length > MAX_LENGTH) {
		return {
			isValid: false,
			error: `Marketing copy must not exceed ${MAX_LENGTH.toLocaleString()} characters`
		};
	}

	// Check if text contains at least some alphabetic characters (not just numbers/symbols)
	const hasAlphabeticContent = /[a-zA-Z]/.test(trimmedText);
	if (!hasAlphabeticContent) {
		return {
			isValid: false,
			error: 'Marketing copy must contain readable text'
		};
	}

	// Check for likely meaningful content (at least one word with 2+ characters)
	const hasWords = /\b[a-zA-Z]{2,}\b/.test(trimmedText);
	if (!hasWords) {
		return {
			isValid: false,
			error: 'Please enter meaningful marketing copy'
		};
	}

	return { isValid: true };
}

/**
 * Sanitizes user input to prevent XSS
 * This is a basic sanitization - server-side sanitization should also be applied
 */
export function sanitizeInput(input: string): string {
	return input
		.replace(/</g, '&lt;')
		.replace(/>/g, '&gt;')
		.replace(/"/g, '&quot;')
		.replace(/'/g, '&#x27;')
		.replace(/\//g, '&#x2F;');
}

/**
 * Normalizes a URL by ensuring it has a protocol
 */
export function normalizeUrl(url: string): string {
	const trimmed = url.trim();
	if (!trimmed.startsWith('http://') && !trimmed.startsWith('https://')) {
		return `https://${trimmed}`;
	}
	return trimmed;
}

// ============================================
// Authentication Validation
// ============================================

/**
 * Validates an email address
 * - Must be a valid email format
 * - Must not be empty
 */
export function validateEmail(email: string): ValidationResult {
	if (!email || email.trim() === '') {
		return {
			isValid: false,
			error: 'Please enter your email address'
		};
	}

	const trimmedEmail = email.trim().toLowerCase();

	// RFC 5322 compliant email regex (simplified)
	const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

	if (!emailRegex.test(trimmedEmail)) {
		return {
			isValid: false,
			error: 'Please enter a valid email address'
		};
	}

	// Check for reasonable length
	if (trimmedEmail.length > 254) {
		return {
			isValid: false,
			error: 'Email address is too long'
		};
	}

	return { isValid: true };
}

/**
 * Password strength requirements
 */
export interface PasswordRequirements {
	minLength: boolean;
	hasUppercase: boolean;
	hasLowercase: boolean;
	hasNumber: boolean;
	hasSpecialChar: boolean;
}

/**
 * Validates a password
 * - Must be at least 8 characters
 * - Must contain at least one uppercase letter
 * - Must contain at least one lowercase letter
 * - Must contain at least one number
 * - Should contain at least one special character (warning only)
 */
export function validatePassword(password: string): ValidationResult & { requirements?: PasswordRequirements } {
	if (!password || password === '') {
		return {
			isValid: false,
			error: 'Please enter a password'
		};
	}

	const MIN_LENGTH = 8;
	const MAX_LENGTH = 128;

	const requirements: PasswordRequirements = {
		minLength: password.length >= MIN_LENGTH,
		hasUppercase: /[A-Z]/.test(password),
		hasLowercase: /[a-z]/.test(password),
		hasNumber: /[0-9]/.test(password),
		hasSpecialChar: /[!@#$%^&*()_+\-=\[\]{};':"\\|,.<>\/?`~]/.test(password)
	};

	if (password.length > MAX_LENGTH) {
		return {
			isValid: false,
			error: 'Password is too long (maximum 128 characters)',
			requirements
		};
	}

	if (!requirements.minLength) {
		return {
			isValid: false,
			error: 'Password must be at least 8 characters long',
			requirements
		};
	}

	if (!requirements.hasUppercase) {
		return {
			isValid: false,
			error: 'Password must contain at least one uppercase letter',
			requirements
		};
	}

	if (!requirements.hasLowercase) {
		return {
			isValid: false,
			error: 'Password must contain at least one lowercase letter',
			requirements
		};
	}

	if (!requirements.hasNumber) {
		return {
			isValid: false,
			error: 'Password must contain at least one number',
			requirements
		};
	}

	return { isValid: true, requirements };
}

/**
 * Validates password confirmation matches
 */
export function validatePasswordConfirmation(password: string, confirmation: string): ValidationResult {
	if (!confirmation || confirmation === '') {
		return {
			isValid: false,
			error: 'Please confirm your password'
		};
	}

	if (password !== confirmation) {
		return {
			isValid: false,
			error: 'Passwords do not match'
		};
	}

	return { isValid: true };
}

/**
 * Validates a user's display name
 * - Must be at least 2 characters
 * - Must not exceed 100 characters
 * - Must contain only valid characters
 */
export function validateName(name: string): ValidationResult {
	if (!name || name.trim() === '') {
		return {
			isValid: false,
			error: 'Please enter your name'
		};
	}

	const trimmedName = name.trim();
	const MIN_LENGTH = 2;
	const MAX_LENGTH = 100;

	if (trimmedName.length < MIN_LENGTH) {
		return {
			isValid: false,
			error: `Name must be at least ${MIN_LENGTH} characters`
		};
	}

	if (trimmedName.length > MAX_LENGTH) {
		return {
			isValid: false,
			error: `Name must not exceed ${MAX_LENGTH} characters`
		};
	}

	// Check for valid characters (letters, spaces, hyphens, apostrophes)
	const validNameRegex = /^[\p{L}\s\-'\.]+$/u;
	if (!validNameRegex.test(trimmedName)) {
		return {
			isValid: false,
			error: 'Name contains invalid characters'
		};
	}

	return { isValid: true };
}

/**
 * Validates login form data
 */
export function validateLoginForm(email: string, password: string): ValidationResult {
	const emailResult = validateEmail(email);
	if (!emailResult.isValid) {
		return emailResult;
	}

	if (!password || password === '') {
		return {
			isValid: false,
			error: 'Please enter your password'
		};
	}

	return { isValid: true };
}

/**
 * Validates registration form data
 */
export function validateRegistrationForm(
	email: string,
	password: string,
	confirmPassword: string,
	name: string
): ValidationResult {
	const nameResult = validateName(name);
	if (!nameResult.isValid) {
		return nameResult;
	}

	const emailResult = validateEmail(email);
	if (!emailResult.isValid) {
		return emailResult;
	}

	const passwordResult = validatePassword(password);
	if (!passwordResult.isValid) {
		return passwordResult;
	}

	const confirmResult = validatePasswordConfirmation(password, confirmPassword);
	if (!confirmResult.isValid) {
		return confirmResult;
	}

	return { isValid: true };
}
