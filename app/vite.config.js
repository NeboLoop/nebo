import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';
import { resolve } from 'path';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],
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
			port: 5173,
		},
		proxy: {
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
			'/apps': {
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
		}
	}
});
