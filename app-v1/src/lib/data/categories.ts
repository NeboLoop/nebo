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
