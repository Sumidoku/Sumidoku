import { registerSW } from 'virtual:pwa-register';

export function registerPwa() {
	registerSW({ immediate: true });
}
