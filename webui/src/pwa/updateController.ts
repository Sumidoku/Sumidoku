export type UpdateServiceWorker = (reloadPage?: boolean) => Promise<void>;

export function createPwaUpdateController() {
	let visible = false;
	let updateSW: UpdateServiceWorker | undefined;
	const listeners = new Set<() => void>();

	function emit() {
		listeners.forEach((listener) => listener());
	}

	return {
		subscribe(listener: () => void) {
			listeners.add(listener);
			return () => {
				listeners.delete(listener);
			};
		},
		getSnapshot() {
			return visible;
		},
		show(update: UpdateServiceWorker) {
			updateSW = update;
			visible = true;
			emit();
		},
		dismiss() {
			visible = false;
			updateSW = undefined;
			emit();
		},
		async reload() {
			if (!updateSW) return;
			await updateSW(true);
		}
	};
}

export const pwaUpdateController = createPwaUpdateController();
