import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig(({ mode }) => ({
	plugins: [
		react(),
		VitePWA({
			strategies: 'generateSW',
			registerType: 'autoUpdate',
			includeAssets: ['apple-touch-icon.png', 'icon.svg'],
			manifest: {
				name: 'Sumidoku',
				short_name: 'Sumidoku',
				start_url: '/',
				scope: '/',
				display: 'standalone',
				background_color: '#100f12',
				theme_color: '#100f12',
				icons: [
					{
						src: '/icons/icon-192.png',
						sizes: '192x192',
						type: 'image/png',
						purpose: 'any'
					},
					{
						src: '/icons/icon-512.png',
						sizes: '512x512',
						type: 'image/png',
						purpose: 'any'
					},
					{
						src: '/icons/icon-maskable-512.png',
						sizes: '512x512',
						type: 'image/png',
						purpose: 'maskable'
					}
				]
			},
			workbox: {
				globPatterns: ['**/*.{js,css,html}'],
				navigateFallback: 'index.html',
				navigateFallbackDenylist: [/^\/api(?:\/|$)/, /^\/opds(?:\/|$)/]
			},
			devOptions: {
				enabled: mode === 'pwa'
			}
		})
	],
	build: {
		outDir: 'build'
	},
	server: {
		proxy: {
			'/api': { target: process.env.SUMIDOKU_DEV_API ?? 'http://localhost:3000', ws: true }
		}
	}
}));
