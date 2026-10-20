import { setupI18n } from '$lib/i18n';

// 全站 SPA：关闭 SSR/预渲染，避免模块顶层的 window/localStorage/matchMedia 在 Node 构建期执行
export const ssr = false;
export const prerender = false;
export const trailingSlash = 'ignore';

// 首屏前就绪初始语言字典，避免看到裸 key
export const load = async () => {
	await setupI18n();
	return {};
};
