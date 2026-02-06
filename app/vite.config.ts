import { defineConfig } from 'vite';
import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
	plugins: [tailwindcss(), sveltekit()],

	resolve: {
		alias: {
			$src: 'src'
		}
	},

	server: {
		host: 'local.nebo.bot',
		port: 5173,
		proxy: {
			// Proxy API requests to Go backend during development
			'/api': {
				target: 'http://local.nebo.bot:27895',
				changeOrigin: true
			},
			// Proxy health check
			'/health': {
				target: 'http://local.nebo.bot:27895',
				changeOrigin: true
			},
			// Proxy subscription plans (public endpoint)
			'/subscription/plans': {
				target: 'http://local.nebo.bot:27895',
				changeOrigin: true
			},
			// Proxy WebSocket connections
			'/ws': {
				target: 'ws://local.nebo.bot:27895',
				ws: true,
				changeOrigin: true
			}
		}
	}
});
