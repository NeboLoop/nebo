export interface Category {
	name: string;
	slug: string;
	emoji: string;
}

export function slugify(name: string): string {
	return name
		.toLowerCase()
		.replace(/\s*&\s*/g, '-and-')
		.replace(/\s+/g, '-')
		.replace(/[^a-z0-9-]/g, '');
}

// Editorial copy per category — the marketing lede that turns a flat result
// list into a category storefront. Mirrors neboai.com's category pages, keyed
// by the canonical category name.
export interface CategoryMeta {
	headline: string;
	lede: string;
}

export const categoryMeta: Record<string, CategoryMeta> = {
	'Run your business': {
		headline: 'Get your business running smoother.',
		lede: 'The unglamorous parts — invoicing, scheduling, client follow-ups, vendor wrangling. The stuff that wins back your Sunday.'
	},
	'Create content': {
		headline: 'Create content that sounds like you.',
		lede: 'Drafts, edits, decks, social posts — in your voice, on your timeline. The work goes faster. The output still feels like you wrote it.'
	},
	'Find customers': {
		headline: 'Reach the right people without the grind.',
		lede: 'The right outreach at the right time. Lead gen, SEO audits, ad copy, follow-ups — the grind that turns strangers into customers.'
	},
	'Manage money': {
		headline: 'Know where your money goes.',
		lede: "Track expenses, catch surprises, plan ahead. No spreadsheets you'll never open again. No apps you'll stop using by March."
	},
	'Get organized': {
		headline: 'Stay on top of everything.',
		lede: 'A briefing before your first meeting. A nudge before you forget. The right file when you need it. Less rummaging, more shipping.'
	},
	'Communicate': {
		headline: 'Say the right thing, faster.',
		lede: 'Better emails, sharper proposals, smoother follow-ups. Get your point across without overthinking every word.'
	},
	'Learn & grow': {
		headline: 'Get better at what you care about.',
		lede: "Study smarter. Prep for the interview. Practice the conversation. Like a patient tutor who's read everything and never gets tired."
	},
	'Research & decide': {
		headline: 'Make decisions with better information.',
		lede: 'Dig into markets, compare options, check facts. Spend less time gathering information and more time acting on it.'
	},
	'Handle documents': {
		headline: 'Move faster through paperwork.',
		lede: 'PDFs read and summarized. Word docs filled in. Spreadsheets explained. The boring file-shuffling done before you finish your coffee.'
	},
	'Build & connect': {
		headline: 'Make your tools work together.',
		lede: "Stitch together the apps you already use. Catch the patterns you keep repeating. Build the little workflow you've been meaning to write — without writing it."
	}
};
