import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';
import { resolve } from 'path';


// WKWebView cold-start fix (2026-08-22): SvelteKit's SPA start imports every
// route-node module TWICE (preload + enter). WebKit/JavaScriptCore has a race
// where the second concurrent import() of an in-flight module can observe the
// module namespace BEFORE evaluation completes, so kit's
// get_navigation_result_from_branch reads `node.component` mid-evaluation and
// the app dies on "Cannot access 'component' before initialization" — dev
// only (the production bundle collapses the modules), WebKit only (Chromium
// serializes). Reproduced deterministically with headless WebKit; memoizing
// the node loaders (single promise per node) eliminates it across repeated
// runs. This transform wraps the GENERATED loader array, so it survives kit
// regenerating .svelte-kit on every dev start.
function webkitNodeDedupe() {
	return {
		name: 'nebo-webkit-node-dedupe',
		apply: 'serve',
		transform(/** @type {string} */ code, /** @type {string} */ id) {
			if (!id.replace(/\\/g, '/').endsWith('.svelte-kit/generated/client/app.js')) return null;
			const wrapped = code.replace(
				/\(\)\s*=>\s*import\('(\.\/nodes\/\d+)'\)/g,
				(/** @type {string} */ _, /** @type {string} */ spec) => `__nebo_once(() => import('${spec}'))`
			);
			if (wrapped === code) return null;
			return {
				code:
					'function __nebo_once(loader) { let p; return () => (p ??= loader()); }\n' +
					wrapped,
				map: null
			};
		}
	};
}

// Shared by dev and preview so `vite preview` can exercise the PRODUCTION
// bundle against the same live backend (WebKit prod-bundle debugging).
const backendProxy = {
			'/api': {
				target: 'http://localhost:27895',
				changeOrigin: true
			},
			'/health': {
				target: 'http://localhost:27895',
				changeOrigin: true
			},
			'/subscription/plans': {
				target: 'http://localhost:27895',
				changeOrigin: true
			},
			// Only proxy app-sidecar sub-paths (/apps/<agent_id>/ui|api|storage|…) to
			// the backend. Bare `/apps` is the SvelteKit installed-apps grid route —
			// proxying it (the old `'/apps'` prefix) shadowed that page in dev. The
			// `^` key is a regex, so it matches /apps/<seg>/… but not bare /apps.
			'^/apps/[^/]+/': {
				target: 'http://localhost:27895',
				changeOrigin: true
			},
			'/sdk': {
				target: 'http://localhost:27895',
				changeOrigin: true
			},
			'/ws': {
				target: 'ws://localhost:27895',
				ws: true,
				changeOrigin: true
			}
};

export default defineConfig({
	plugins: [tailwindcss(), sveltekit(), webkitNodeDedupe()],
	resolve: {
		alias: {
			'daisyui/theme': resolve('node_modules/daisyui/theme/index.js'),
			daisyui: resolve('node_modules/daisyui/index.js'),
		}
	},
	server: {
		strictPort: true,
		hmr: {
			protocol: 'ws',
			host: 'localhost',
			// Follows the dev port so a second dev server (e.g. a worktree preview
			// on 5174) doesn't point its HMR socket at the first one's.
			port: Number(process.env.VITE_DEV_PORT ?? 5173),
		},
		proxy: backendProxy
	},
	preview: {
		proxy: backendProxy
	}
});
