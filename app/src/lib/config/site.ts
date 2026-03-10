/**
 * Site Configuration
 * ==================
 * This is the SINGLE source of truth for your site's branding and metadata.
 * Update these values once and they propagate everywhere:
 * - SEO meta tags
 * - Footer
 * - Navigation
 * - Email templates
 * - OG images
 */

export const site = {
	// ============================================
	// REQUIRED: Update these for your site
	// ============================================

	name: 'Nebo',
	tagline: 'Your AI Agent Platform',
	description: 'Build and deploy AI agents with tool use, multi-provider support, and channel integrations.',

	// Your production URL (no trailing slash)
	url: 'https://example.com',

	// Support email shown in footer and emails
	supportEmail: 'support@example.com',

	// ============================================
	// SEO & Social
	// ============================================

	// Default OG image (place at /static/images/og-default.png, 1200x630px)
	ogImage: '/images/og-default.png',

	// Twitter/X handle (with @)
	twitter: '@yourhandle',

	// Default keywords for SEO
	keywords: ['saas', 'startup', 'boilerplate'],

	// Language/locale
	locale: 'en_US',

	// ============================================
	// Social Links (leave empty to hide)
	// ============================================

	social: {
		twitter: '', // https://twitter.com/yourhandle
		github: '', // https://github.com/yourorg
		linkedin: '', // https://linkedin.com/company/yourcompany
		discord: '' // https://discord.gg/yourserver
	},

	// ============================================
	// Legal Pages
	// ============================================

	legal: {
		companyName: 'Your Company, Inc.',
		companyAddress: '123 Main St, City, State 12345',
		privacyUrl: '/privacy',
		termsUrl: '/terms'
	}
} as const;

// Type export for use in components
export type SiteConfig = typeof site;
