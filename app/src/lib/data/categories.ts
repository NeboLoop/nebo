export interface Category {
	name: string;
	slug: string;
	emoji: string;
	gradient: string;
}

export const categories: Category[] = [
	{ name: 'Productivity', slug: 'productivity', emoji: '⚡', gradient: 'from-yellow-400 to-amber-500' },
	{ name: 'Finance', slug: 'finance', emoji: '💰', gradient: 'from-emerald-400 to-green-600' },
	{ name: 'Sales', slug: 'sales', emoji: '📈', gradient: 'from-blue-400 to-indigo-600' },
	{ name: 'Marketing', slug: 'marketing', emoji: '📣', gradient: 'from-pink-400 to-rose-600' },
	{ name: 'Operations', slug: 'operations', emoji: '⚙️', gradient: 'from-violet-500 to-purple-700' },
	{ name: 'HR & Recruiting', slug: 'hr-and-recruiting', emoji: '🤝', gradient: 'from-orange-400 to-red-500' },
	{ name: 'Education', slug: 'education', emoji: '🎓', gradient: 'from-violet-400 to-purple-600' },
	{ name: 'Health', slug: 'health', emoji: '❤️', gradient: 'from-red-400 to-rose-600' },
	{ name: 'Creative', slug: 'creative', emoji: '🎨', gradient: 'from-fuchsia-400 to-pink-600' },
	{ name: 'Development', slug: 'development', emoji: '🛠️', gradient: 'from-cyan-400 to-sky-600' },
	{ name: 'Security', slug: 'security', emoji: '🔒', gradient: 'from-blue-600 to-indigo-800' },
	{ name: 'Data & Analytics', slug: 'data-and-analytics', emoji: '📊', gradient: 'from-teal-400 to-emerald-600' },
	{ name: 'Connectors', slug: 'connectors', emoji: '🔗', gradient: 'from-sky-400 to-blue-600' },
	{ name: 'Communication', slug: 'communication', emoji: '💬', gradient: 'from-indigo-400 to-violet-600' },
	{ name: 'Real Estate', slug: 'real-estate', emoji: '🏠', gradient: 'from-amber-400 to-orange-600' },
	{ name: 'Legal', slug: 'legal', emoji: '⚖️', gradient: 'from-slate-600 to-blue-800' },
	{ name: 'E-commerce', slug: 'e-commerce', emoji: '🛒', gradient: 'from-lime-400 to-green-600' },
	{ name: 'Personal', slug: 'personal', emoji: '🧘', gradient: 'from-rose-300 to-pink-500' },
	{ name: 'Research', slug: 'research', emoji: '🔬', gradient: 'from-cyan-500 to-blue-700' },
	{ name: 'Automation', slug: 'automation', emoji: '🤖', gradient: 'from-green-500 to-emerald-700' }
];

export function slugify(name: string): string {
	return name
		.toLowerCase()
		.replace(/\s*&\s*/g, '-and-')
		.replace(/\s+/g, '-')
		.replace(/[^a-z0-9-]/g, '');
}

export function categoryBySlug(slug: string): Category | undefined {
	return categories.find((c) => c.slug === slug);
}

export function categoryByName(name: string): Category | undefined {
	return categories.find((c) => c.name === name);
}
