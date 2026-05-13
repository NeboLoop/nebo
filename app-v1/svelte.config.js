import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	// Consult https://svelte.dev/docs/kit/integrations
	// for more information about preprocessors
	preprocess: vitePreprocess(),


	kit: {
		alias: {
			$src: 'src'
		},
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			fallback: '200.html', // SPA fallback for non-prerendered routes
			precompress: false,
			strict: false
		}),
		prerender: {
			entries: ['*'], // Prerender all discoverable routes
			crawl: true, // Automatically crawl links to find routes
			handleHttpError: 'ignore',
			handleMissingId: 'ignore',
			handleUnseenRoutes: 'ignore' // Ignore dynamic routes without data
		}
	}
};

export default config;
