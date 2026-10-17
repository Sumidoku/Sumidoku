import { useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';

export interface ToastItem {
	id: number;
	message: string;
	kind: 'error' | 'info';
}

let items: ToastItem[] = [];
const listeners = new Set<() => void>();
let seq = 0;

function emit() {
	listeners.forEach((l) => l());
}

// 同一消息 2s 内的重复推送直接吞掉：失败重试/轮询类报错会连发，堆一屏相同 toast 没信息量
let lastPush = { message: '', at: 0 };

export function pushToast(message: string, kind: 'error' | 'info' = 'error') {
	const now = Date.now();
	if (message === lastPush.message && now - lastPush.at < 2000) return;
	lastPush = { message, at: now };
	const id = ++seq;
	items = [...items, { id, message, kind }];
	emit();
	setTimeout(() => {
		items = items.filter((t) => t.id !== id);
		emit();
	}, 3200);
}

function subscribe(l: () => void) {
	listeners.add(l);
	return () => {
		listeners.delete(l);
	};
}

/** 全局 toast 出口，挂在 main.tsx 一次即可 */
export function Toaster() {
	const toasts = useSyncExternalStore(subscribe, () => items);
	return createPortal(
		<div className="toaster" aria-live="polite">
			{toasts.map((t) => (
				<div key={t.id} className={t.kind === 'info' ? 'toast toast--info' : 'toast'}>
					{t.message}
				</div>
			))}
		</div>,
		document.body
	);
}
