// 手写 service worker：SPA 离线外壳。无构建期预缓存（资源名带 hash），改用运行时缓存：
// - 导航请求：网络优先，断网回退缓存的 index.html
// - /_app/ 下带 hash 的不可变产物：缓存优先
// - /api、/opds：永远走网络（不缓存，避免把后端喂成 HTML 外壳）
// bump CACHE 版本即可让旧缓存在 activate 时清掉。
const CACHE = 'sumidoku-shell-v1';
const SHELL = ['/', '/index.html', '/icon.svg', '/manifest.webmanifest'];

self.addEventListener('install', (event) => {
	event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)));
});

self.addEventListener('activate', (event) => {
	event.waitUntil(
		caches
			.keys()
			.then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
			.then(() => self.clients.claim())
	);
});

// 页面点「重新加载」时发来，立即接管以便下次导航用新资源
self.addEventListener('message', (event) => {
	if (event.data === 'SKIP_WAITING') self.skipWaiting();
});

self.addEventListener('fetch', (event) => {
	const { request } = event;
	if (request.method !== 'GET') return;
	const url = new URL(request.url);
	if (url.origin !== self.location.origin) return;
	if (url.pathname.startsWith('/api') || url.pathname.startsWith('/opds')) return;

	if (request.mode === 'navigate') {
		event.respondWith(fetch(request).catch(() => caches.match('/index.html')));
		return;
	}

	if (url.pathname.startsWith('/_app/')) {
		event.respondWith(
			caches.match(request).then(
				(hit) =>
					hit ||
					fetch(request).then((res) => {
						const copy = res.clone();
						caches.open(CACHE).then((cache) => cache.put(request, copy));
						return res;
					})
			)
		);
	}
});
