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
		// Dev AND build: the production bundle has the same double import of
		// every route node, and iPhone-over-tunnel first loads hit it
		// (2026-09-15: "pu is not a function" inside ChatPane's chunk, WebKit
		// only, probabilistic — the owner's phone sat on the boot spinner).
		transform(/** @type {string} */ code, /** @type {string} */ id) {
			// dev reads generated/client, build reads generated/client-optimized
			if (!/\.svelte-kit\/generated\/client(-optimized)?\/app\.js$/.test(id.replace(/\\/g, '/'))) return null;
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

// KaTeX ships each of its 60-odd font faces three times — woff2, woff and a
// bare ttf — and its stylesheet names all three, so Vite copies all three into
// the bundle: about 730 KB of files no browser we run on will ever request.
// Every webview here (Chromium, WebKit, the Tauri shell) has supported woff2
// for years, so the other two `src` entries are dropped from the stylesheet on
// the way through, and the files stop being referenced and stop being emitted.
function katexWoff2Only() {
	return {
		name: 'nebo-katex-woff2-only',
		// Before `vite:css`, which rewrites every `url()` into a hashed asset
		// reference — after that the `.woff` in the path is gone and there is
		// nothing left to match on.
		enforce: 'pre',
		transform(/** @type {string} */ code, /** @type {string} */ id) {
			if (!/katex(\.min)?\.css$/.test(id.replace(/\\/g, '/').split('?')[0])) return null;
			// Each face reads `src: url(x.woff2) format("woff2"), url(x.woff) …`.
			// Keep the first pair, drop the rest of the list.
			const trimmed = code.replace(
				/src:\s*(url\([^)]*\.woff2\)\s*format\("woff2"\))[^;]*;/g,
				'src: $1;'
			);
			if (trimmed === code) return null;
			return { code: trimmed, map: null };
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
	plugins: [tailwindcss(), sveltekit(), webkitNodeDedupe(), katexWoff2Only()],
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
