import { toast } from 'svelte-sonner';

// 同一消息 2s 内的重复推送直接吞掉：失败重试/轮询类报错会连发，堆一屏相同 toast 没信息量
let lastPush = { message: '', at: 0 };

/** 全局 toast 出口；<Toaster/>（svelte-sonner）在根布局挂一次即可 */
export function pushToast(message: string, kind: 'error' | 'info' = 'error') {
	const now = Date.now();
	if (message === lastPush.message && now - lastPush.at < 2000) return;
	lastPush = { message, at: now };
	if (kind === 'info') {
		toast(message, { duration: 3200 });
	} else {
		toast.error(message, { duration: 3200 });
	}
}
