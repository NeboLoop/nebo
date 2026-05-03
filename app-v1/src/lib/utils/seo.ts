/**
 * SEO Utilities
 * Provides consistent meta tags across all pages
 *
 * NOTE: Site configuration is centralized in $lib/config/site.ts
 */

import { site } from '$lib/config/site';

export interface SEOConfig {
	title: string;
	description: string;
	keywords?: string[];
	image?: string;
	url?: string;
	type?: 'website' | 'article' | 'product';
	noindex?: boolean;
	canonical?: string;
	author?: string;
	publishedTime?: string;
	modifiedTime?: string;
}

export interface SEOOutput {
	title: string;
	description: string;
	keywords: string;
	image: string;
	url: string;
	type: string;
	noindex: boolean;
	canonical: string;
	author: string;
	publishedTime?: string;
	modifiedTime?: string;
	siteName: string;
	twitterHandle: string;
	locale: string;
}

// Re-export site config for convenience
export { site };

// Derived constants from centralized config
const SITE_NAME = site.name;
const SITE_URL = site.url;
const DEFAULT_IMAGE = `${SITE_URL}${site.ogImage}`;
const TWITTER_HANDLE = site.twitter;
const DEFAULT_KEYWORDS = site.keywords;

/**
 * Generate comprehensive SEO meta data
 */
export function setSEO(config: SEOConfig): SEOOutput {
	const {
		title,
		description,
		keywords = [],
		image = DEFAULT_IMAGE,
		url = '',
		type = 'website',
		noindex = false,
		canonical = '',
		author = '',
		publishedTime,
		modifiedTime
	} = config;

	// Combine default and custom keywords
	const allKeywords = [...new Set([...DEFAULT_KEYWORDS, ...keywords])];

	return {
		title: `${title} | ${SITE_NAME}`,
		description,
		keywords: allKeywords.join(', '),
		image: image.startsWith('http') ? image : `${SITE_URL}${image}`,
		url: url ? (url.startsWith('http') ? url : `${SITE_URL}${url}`) : SITE_URL,
		type,
		noindex,
		canonical: canonical || (url ? `${SITE_URL}${url}` : SITE_URL),
		author,
		publishedTime,
		modifiedTime,
		siteName: SITE_NAME,
		twitterHandle: TWITTER_HANDLE,
		locale: 'en_US'
	};
}

/**
 * Generate JSON-LD Organization schema
 */
export function getOrganizationSchema() {
	const sameAs = [
		site.social.twitter,
		site.social.github,
		site.social.linkedin,
		site.social.discord
	].filter(Boolean);

	return {
		'@context': 'https://schema.org',
		'@type': 'Organization',
		name: SITE_NAME,
		url: SITE_URL,
		logo: `${SITE_URL}/images/logo.svg`,
		sameAs,
		description: site.description,
		contactPoint: {
			'@type': 'ContactPoint',
			contactType: 'customer service',
			email: site.supportEmail
		}
	};
}

/**
 * Generate JSON-LD WebSite schema with search action
 */
export function getWebsiteSchema() {
	return {
		'@context': 'https://schema.org',
		'@type': 'WebSite',
		name: SITE_NAME,
		url: SITE_URL,
		potentialAction: {
			'@type': 'SearchAction',
			target: {
				'@type': 'EntryPoint',
				urlTemplate: `${SITE_URL}/search?q={search_term_string}`
			},
			'query-input': 'required name=search_term_string'
		}
	};
}

/**
 * Generate JSON-LD SoftwareApplication schema
 */
export function getSoftwareSchema() {
	return {
		'@context': 'https://schema.org',
		'@type': 'SoftwareApplication',
		name: SITE_NAME,
		applicationCategory: 'BusinessApplication',
		operatingSystem: 'Web',
		offers: {
			'@type': 'Offer',
			price: '0',
			priceCurrency: 'USD',
			description: 'Free tier available'
		}
	};
}

/**
 * Generate JSON-LD BreadcrumbList schema
 */
export function getBreadcrumbSchema(items: Array<{ name: string; url: string }>) {
	return {
		'@context': 'https://schema.org',
		'@type': 'BreadcrumbList',
		itemListElement: items.map((item, index) => ({
			'@type': 'ListItem',
			position: index + 1,
			name: item.name,
			item: item.url.startsWith('http') ? item.url : `${SITE_URL}${item.url}`
		}))
	};
}

/**
 * Generate JSON-LD FAQ schema
 */
export function getFAQSchema(faqs: Array<{ question: string; answer: string }>) {
	return {
		'@context': 'https://schema.org',
		'@type': 'FAQPage',
		mainEntity: faqs.map((faq) => ({
			'@type': 'Question',
			name: faq.question,
			acceptedAnswer: {
				'@type': 'Answer',
				text: faq.answer
			}
		}))
	};
}

/**
 * Generate JSON-LD Article schema
 */
export function getArticleSchema(article: {
	title: string;
	description: string;
	image: string;
	datePublished: string;
	dateModified?: string;
	author: string;
	url: string;
}) {
	return {
		'@context': 'https://schema.org',
		'@type': 'Article',
		headline: article.title,
		description: article.description,
		image: article.image.startsWith('http') ? article.image : `${SITE_URL}${article.image}`,
		datePublished: article.datePublished,
		dateModified: article.dateModified || article.datePublished,
		author: {
			'@type': 'Person',
			name: article.author
		},
		publisher: {
			'@type': 'Organization',
			name: SITE_NAME,
			logo: {
				'@type': 'ImageObject',
				url: `${SITE_URL}/images/logo.svg`
			}
		},
		mainEntityOfPage: {
			'@type': 'WebPage',
			'@id': article.url.startsWith('http') ? article.url : `${SITE_URL}${article.url}`
		}
	};
}
