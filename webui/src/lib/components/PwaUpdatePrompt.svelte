<script lang="ts">
	import { onMount } from 'svelte';
	import { _ } from 'svelte-i18n';
	import { Button } from '$lib/components/ui/button';

	// 手写 SW 注册（不依赖 PWA 插件）。只在生产构建注册，避免开发期干扰 HMR。
	// 新 SW 装好且已有旧 SW 控制页面时，needRefresh 翻真，由用户点「重新加载」才接管并刷新。
	let needRefresh = $state(false);
	let waiting: ServiceWorker | null = null;

	function reload() {
		if (!waiting) return;
		// 接管后 controllerchange 触发，刷新到新资源
		navigator.serviceWorker.addEventListener('controllerchange', () => location.reload(), {
			once: true
		});
		waiting.postMessage('SKIP_WAITING');
	}

	function dismiss() {
		needRefresh = false;
	}

	onMount(() => {
		if (!import.meta.env.PROD || !('serviceWorker' in navigator)) return;
		let reg: ServiceWorkerRegistration | undefined;
		navigator.serviceWorker
			.register('/sw.js')
			.then((registration) => {
				reg = registration;
				// 已有等待中的 SW（上次会话装好但没接管）
				if (registration.waiting && navigator.serviceWorker.controller) {
					waiting = registration.waiting;
					needRefresh = true;
				}
				registration.addEventListener('updatefound', () => {
					const next = registration.installing;
					if (!next) return;
					next.addEventListener('statechange', () => {
						if (next.state === 'installed' && navigator.serviceWorker.controller) {
							waiting = next;
							needRefresh = true;
						}
					});
				});
			})
			.catch(() => {
				// 注册失败（如不支持或离线首访）不影响应用使用
			});
		return () => reg; // 保留引用，无需显式注销
	});
</script>

{#if needRefresh}
	<div
		class="fixed inset-x-0 bottom-4 z-50 mx-auto flex w-[min(28rem,calc(100%-2rem))] items-center gap-3 rounded-lg border border-border bg-popover px-4 py-3 text-popover-foreground shadow-lg"
		role="region"
		aria-label={$_('pwa.updateLabel')}
	>
		<span class="flex-1 text-sm" role="status">{$_('pwa.updateAvailable')}</span>
		<div class="flex shrink-0 gap-2">
			<Button size="sm" onclick={reload}>{$_('pwa.reload')}</Button>
			<Button variant="ghost" size="sm" onclick={dismiss}>{$_('pwa.later')}</Button>
		</div>
	</div>
{/if}
