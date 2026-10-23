import { registerSW } from 'virtual:pwa-register';
import { pwaUpdateController } from './updateController';

export function registerPwa() {
	const updateSW = registerSW({
		onNeedRefresh: () => pwaUpdateController.show(updateSW)
	});
}
