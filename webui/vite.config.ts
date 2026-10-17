import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		await sveltekit({
			adapter: adapter({
				pages: 'build',
				assets: 'build',
				fallback: '200.html'
			})
		})
	],
	server: {
		proxy: {
			'/api': 'http://localhost:3000'
		}
	}
});
