import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import test, { after, before } from 'node:test';
import { build } from 'vite';

const root = fileURLToPath(new URL('../', import.meta.url));
let outDir: string;

before(async () => {
	outDir = await mkdtemp(path.join(tmpdir(), 'sumidoku-pwa-'));
	await build({
		root,
		configFile: path.join(root, 'vite.config.ts'),
		logLevel: 'silent',
		build: {
			outDir,
			emptyOutDir: true
		}
	});
});

after(async () => {
	if (outDir) await rm(outDir, { recursive: true, force: true });
});

function pngSize(bytes: Buffer) {
	assert.equal(bytes.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
	assert.equal(bytes.subarray(12, 16).toString('ascii'), 'IHDR');
	return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

test('build emits install metadata with matching icons and one manifest link', async () => {
	const manifest = JSON.parse(await readFile(path.join(outDir, 'manifest.webmanifest'), 'utf8'));
	assert.equal(manifest.name, 'Sumidoku');
	assert.equal(manifest.short_name, 'Sumidoku');
	assert.equal(manifest.start_url, '/');
	assert.equal(manifest.scope, '/');
	assert.equal(manifest.display, 'standalone');
	assert.equal(manifest.background_color, '#100f12');
	assert.equal(manifest.theme_color, '#100f12');
	assert.deepEqual(manifest.icons, [
		{ src: '/icons/icon-192.png', sizes: '192x192', type: 'image/png', purpose: 'any' },
		{ src: '/icons/icon-512.png', sizes: '512x512', type: 'image/png', purpose: 'any' },
		{ src: '/icons/icon-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' }
	]);
	for (const icon of manifest.icons) {
		const [width, height] = icon.sizes.split('x').map(Number);
		assert.deepEqual(pngSize(await readFile(path.join(outDir, icon.src))), { width, height });
	}
	assert.deepEqual(pngSize(await readFile(path.join(outDir, 'apple-touch-icon.png'))), {
		width: 180,
		height: 180
	});

	const index = await readFile(path.join(outDir, 'index.html'), 'utf8');
	const manifestLinks = index.match(/<link rel="manifest"[^>]*>/g) ?? [];
	assert.equal(manifestLinks.length, 1);
	assert.match(manifestLinks[0], /href="\/manifest.webmanifest"/);
	assert.match(index, /<meta name="theme-color" content="#100f12"/);
	assert.match(index, /<link rel="apple-touch-icon" href="\/apple-touch-icon.png"/);
	assert.doesNotMatch(index, /registerSW\.js/);
});

test('build includes the Workbox runtime license notice', async () => {
	const notice = await readFile(path.join(outDir, 'licenses/workbox.txt'), 'utf8');
	assert.match(notice, /Copyright 2018 Google LLC/);
	assert.match(notice, /Permission is hereby granted/);
});

interface PrecacheEntry {
	url: string;
	revision: string | null;
}

interface NavigationOptions {
	denylist?: RegExp[];
}

test('service worker precaches only the shell and excludes API and OPDS navigations', async () => {
	const source = await readFile(path.join(outDir, 'sw.js'), 'utf8');
	const precacheBatches: PrecacheEntry[][] = [];
	const registeredRoutes: unknown[] = [];
	const navigationRoutes: { handler: string; options: NavigationOptions }[] = [];
	let cleanups = 0;
	const workbox = {
		precacheAndRoute(entries: PrecacheEntry[]) {
			precacheBatches.push(entries);
		},
		cleanupOutdatedCaches() {
			cleanups += 1;
		},
		createHandlerBoundToURL(handler: string) {
			return handler;
		},
		NavigationRoute: class {
			constructor(handler: string, options: NavigationOptions) {
				navigationRoutes.push({ handler, options });
			}
		},
		registerRoute(route: unknown) {
			registeredRoutes.push(route);
		},
		clientsClaim() {}
	};
	const context: Record<string, unknown> = {
		define: (_dependencies: string[], factory: (workboxApi: typeof workbox) => void) => factory(workbox),
		addEventListener() {},
		skipWaiting() {}
	};
	context.self = context;
	vm.runInNewContext(source, context, { filename: 'sw.js' });

	assert.equal(precacheBatches.length, 1);
	const urls = precacheBatches[0].map((entry) => entry.url).sort();
	const staticFiles = [
		'index.html',
		'manifest.webmanifest',
		'icon.svg',
		'apple-touch-icon.png',
		'icons/icon-192.png',
		'icons/icon-512.png',
		'icons/icon-maskable-512.png'
	];
	for (const file of staticFiles) assert.ok(urls.includes(file), `${file} is precached`);
	assert.ok(urls.some((url) => /^assets\/.+\.js$/.test(url)), 'built JavaScript is precached');
	assert.ok(urls.some((url) => /^assets\/.+\.css$/.test(url)), 'built CSS is precached');
	for (const url of urls) {
		assert.ok(
			staticFiles.includes(url) || /^assets\/.+\.(?:js|css)$/.test(url),
			`unexpected precache entry: ${url}`
		);
	}
	assert.equal(cleanups, 1);
	assert.equal(registeredRoutes.length, 1, 'only the navigation route is registered');
	assert.equal(navigationRoutes.length, 1);
	assert.equal(navigationRoutes[0].handler, 'index.html');
	const denylist = navigationRoutes[0].options.denylist ?? [];
	const denied = (urlPath: string) => denylist.some((pattern) => pattern.test(urlPath));
	for (const urlPath of ['/api', '/api/', '/api/v1/manga', '/opds', '/opds/', '/opds/catalog']) {
		assert.equal(denied(urlPath), true, `${urlPath} bypasses the SPA fallback`);
	}
	for (const urlPath of ['/apiary', '/opdsomething', '/library', '/manga/1']) {
		assert.equal(denied(urlPath), false, `${urlPath} keeps SPA navigation`);
	}
});
