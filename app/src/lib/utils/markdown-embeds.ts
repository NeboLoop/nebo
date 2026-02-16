/**
 * Marked extension for auto-embedding videos and rich media.
 * Detects YouTube, Vimeo, and X/Twitter URLs and converts them to embeds.
 */
import type { MarkedExtension, Tokens } from 'marked';

// ===== URL Patterns =====

const YOUTUBE_REGEX =
	/(?:https?:\/\/)?(?:www\.)?(?:youtube\.com\/watch\?v=|youtu\.be\/|youtube\.com\/embed\/|youtube\.com\/shorts\/)([a-zA-Z0-9_-]{11})(?:[?&]\S*)?/;

const VIMEO_REGEX =
	/(?:https?:\/\/)?(?:www\.)?vimeo\.com\/(\d+)(?:[?#/]\S*)?/;

const TWITTER_REGEX =
	/(?:https?:\/\/)?(?:www\.)?(?:twitter\.com|x\.com)\/(\w+)\/status\/(\d+)(?:\?\S*)?/;

// ===== Helpers =====

function extractYouTubeId(url: string): string | null {
	const match = url.match(YOUTUBE_REGEX);
	return match ? match[1] : null;
}

function extractVimeoId(url: string): string | null {
	const match = url.match(VIMEO_REGEX);
	return match ? match[1] : null;
}

function extractTweetInfo(url: string): { user: string; id: string } | null {
	const match = url.match(TWITTER_REGEX);
	return match ? { user: match[1], id: match[2] } : null;
}

// ===== Renderers =====

function youtubeEmbed(videoId: string): string {
	return `<div class="embed-container embed-video"><iframe src="https://www.youtube.com/embed/${videoId}" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen loading="lazy"></iframe></div>`;
}

function vimeoEmbed(videoId: string): string {
	return `<div class="embed-container embed-video"><iframe src="https://player.vimeo.com/video/${videoId}?dnt=1" frameborder="0" allow="autoplay; fullscreen; picture-in-picture" allowfullscreen loading="lazy"></iframe></div>`;
}

function tweetEmbed(user: string, tweetId: string, originalUrl: string): string {
	// X's official embed blockquote format — widget.js scans for
	// class="twitter-tweet" and hydrates into the full interactive card.
	// The href MUST use twitter.com (not x.com) for widget.js compat.
	const canonicalUrl = `https://twitter.com/${user}/status/${tweetId}`;
	return `<div class="embed-container embed-tweet"><blockquote class="twitter-tweet" data-dnt="true" data-theme="dark"><p></p>&mdash; @${user} <a href="${canonicalUrl}">${canonicalUrl}</a></blockquote></div>`;
}

// ===== Check if a paragraph is JUST a URL (possibly wrapped in <p> by marked) =====

function isStandaloneUrl(text: string): boolean {
	const trimmed = text.trim();
	// Check if the entire token text is a single URL
	return /^https?:\/\/\S+$/.test(trimmed);
}

// ===== Marked Extension =====

/**
 * Creates a marked extension that auto-embeds media URLs.
 * 
 * When a paragraph contains ONLY a YouTube, Vimeo, or X/Twitter URL,
 * it gets replaced with the appropriate embed. URLs inside regular
 * text paragraphs are left as normal links.
 */
export function embedExtension(): MarkedExtension {
	return {
		renderer: {
			// Override paragraph rendering to detect standalone URLs
			paragraph(this: unknown, token: Tokens.Paragraph): string | false {
				const text = token.raw?.trim() || '';

				// Only transform if the paragraph is a standalone URL
				if (!isStandaloneUrl(text)) return false;

				const url = text;

				// YouTube
				const ytId = extractYouTubeId(url);
				if (ytId) return youtubeEmbed(ytId);

				// Vimeo
				const vimeoId = extractVimeoId(url);
				if (vimeoId) return vimeoEmbed(vimeoId);

				// X/Twitter
				const tweet = extractTweetInfo(url);
				if (tweet) return tweetEmbed(tweet.user, tweet.id, url);

				// Not a recognized embed URL — fall through to default
				return false;
			},

			// Also catch links that are the sole content of a paragraph
			link(this: unknown, token: Tokens.Link): string | false {
				const href = token.href || '';
				const text = token.text || '';

				// Only auto-embed if the link text IS the URL (i.e., auto-linked)
				if (text !== href) return false;

				// YouTube
				const ytId = extractYouTubeId(href);
				if (ytId) return youtubeEmbed(ytId);

				// Vimeo
				const vimeoId = extractVimeoId(href);
				if (vimeoId) return vimeoEmbed(vimeoId);

				// X/Twitter
				const tweet = extractTweetInfo(href);
				if (tweet) return tweetEmbed(tweet.user, tweet.id, href);

				return false;
			}
		}
	};
}
