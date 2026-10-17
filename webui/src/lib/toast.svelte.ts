export interface ToastItem {
	id: number;
	message: string;
	kind: 'error' | 'info';
}

export const toastState = $state<{ items: ToastItem[] }>({ items: [] });

let seq = 0;

export function pushToast(message: string, kind: 'error' | 'info' = 'error') {
	const id = ++seq;
	toastState.items.push({ id, message, kind });
	setTimeout(() => {
		const i = toastState.items.findIndex((t) => t.id === id);
		if (i >= 0) toastState.items.splice(i, 1);
	}, 3200);
}
