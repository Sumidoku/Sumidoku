import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		tailwindcss(),
		await sveltekit({
			preprocess: vitePreprocess(),
			// SPA 模式：全量 index.html 回退，构建产物落在 build/，与 rust-embed 的 #[folder] 一致
			adapter: adapter({ fallback: 'index.html' }),
			// SvelteKit 3 默认移除了 $lib，但 shadcn-svelte 生成的组件仍用 $lib，用官方逃生口保留
			alias: { $lib: 'src/lib' }
		})
		// PWA 不走插件：@vite-pwa/sveltekit 不支持 SvelteKit 3，plain vite-plugin-pwa 又把 SW 生成到错误目录。
		// 改用 static/ 下手写的 manifest.webmanifest + sw.js（见 src/lib/components/PwaUpdatePrompt.svelte 注册）。
	],
	server: {
		proxy: {
			// ws: true 让下载/更新两条 WebSocket 通道在开发代理下也能连上后端
			'/api': { target: 'http://localhost:3000', ws: true },
			'/opds': 'http://localhost:3000'
		}
	}
});
