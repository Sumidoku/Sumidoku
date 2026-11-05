import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';
import type { ViteDevServer } from 'vite';

async function withVite<T>(run: (vite: ViteDevServer) => Promise<T>): Promise<T> {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		return await run(vite);
	} finally {
		await vite.close();
	}
}

type OverrideStore = typeof import('../src/pages/reader/overrideStore.ts');
type FiltersModule = typeof import('../src/pages/reader/filters.ts');

function withStorage(run: (values: Map<string, string>) => void) {
	const values = new Map<string, string>();
	const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
	Object.defineProperty(globalThis, 'localStorage', {
		configurable: true,
		value: {
			getItem: (key: string) => values.get(key) ?? null,
			setItem: (key: string, value: string) => values.set(key, String(value)),
			removeItem: (key: string) => values.delete(key),
			key: (i: number) => [...values.keys()][i] ?? null,
			get length() {
				return values.size;
			}
		}
	});
	try {
		run(values);
	} finally {
		if (original) Object.defineProperty(globalThis, 'localStorage', original);
		else Reflect.deleteProperty(globalThis, 'localStorage');
	}
}

test('content-setting overrides round-trip through per-manga keys', async () => {
	await withVite(async (vite) => {
		const store: OverrideStore = await vite.ssrLoadModule('/src/pages/reader/overrideStore.ts');
		const { DEFAULT_FILTERS }: FiltersModule = await vite.ssrLoadModule('/src/pages/reader/filters.ts');
		withStorage((values) => {
			assert.equal(store.writeModeOverride(7, 'webtoon', 'paged'), 'webtoon');
			assert.equal(store.writeDirOverride(7, 'rtl', 'ltr'), 'rtl');
			assert.equal(store.writeLayoutOverride(7, 'double', 'single'), 'double');
			assert.equal(store.writeScaleOverride(7, 'width', 'screen'), 'width');
			assert.equal(store.writeStretchOverride(7, true, false), true);
			assert.equal(store.writeSplitOverride(7, true, false), true);
			const filters = { ...DEFAULT_FILTERS, grayscale: true };
			assert.equal(store.writeFiltersOverride(7, filters, DEFAULT_FILTERS), filters);
			assert.equal(store.readModeOverride(7), 'webtoon');
			assert.equal(store.readDirOverride(7), 'rtl');
			assert.equal(store.readLayoutOverride(7), 'double');
			assert.equal(store.readScaleOverride(7), 'width');
			assert.equal(store.readStretchOverride(7), true);
			assert.equal(store.readSplitOverride(7), true);
			assert.equal(store.readFiltersOverride(7)?.grayscale, true);
			// Every write landed on the manga-scoped key, none on the global one
			assert.deepEqual(
				[...values.keys()].sort(),
				[
					'sumidoku.reader.dir.7',
					'sumidoku.reader.filters.7',
					'sumidoku.reader.layout.7',
					'sumidoku.reader.mode.7',
					'sumidoku.reader.scale.7',
					'sumidoku.reader.split.7',
					'sumidoku.reader.stretch.7'
				]
			);
		});
	});
});

test('writing the value the resolution already produces deletes the key', async () => {
	await withVite(async (vite) => {
		const store: OverrideStore = await vite.ssrLoadModule('/src/pages/reader/overrideStore.ts');
		const { DEFAULT_FILTERS }: FiltersModule = await vite.ssrLoadModule('/src/pages/reader/filters.ts');
		withStorage((values) => {
			// An existing override disappears when it drifts back to the default
			store.writeLayoutOverride(7, 'double', 'single');
			assert.equal(store.writeLayoutOverride(7, 'single', 'single'), null);
			assert.equal(store.readLayoutOverride(7), null);
			// ...and matching the default never stores a redundant entry
			assert.equal(store.writeScaleOverride(7, 'screen', 'screen'), null);
			assert.equal(store.writeStretchOverride(7, false, false), null);
			assert.equal(store.writeSplitOverride(7, false, false), null);
			assert.equal(store.writeFiltersOverride(7, DEFAULT_FILTERS, DEFAULT_FILTERS), null);
			assert.deepEqual([...values.keys()], []);
		});
	});
});

test('mode and direction overrides drop only against a known metadata value', async () => {
	await withVite(async (vite) => {
		const store: OverrideStore = await vite.ssrLoadModule('/src/pages/reader/overrideStore.ts');
		withStorage(() => {
			// Matching the metadata clears the override so future metadata edits apply
			store.writeModeOverride(7, 'webtoon', 'paged');
			assert.equal(store.writeModeOverride(7, 'paged', 'paged'), null);
			assert.equal(store.readModeOverride(7), null);
			store.writeDirOverride(7, 'ltr', 'rtl');
			assert.equal(store.writeDirOverride(7, 'rtl', 'rtl'), null);
			assert.equal(store.readDirOverride(7), null);
			// While the metadata is unknown (still loading) the override must stick,
			// even when it equals the global default — otherwise the arriving
			// metadata would silently flip the reader
			assert.equal(store.writeModeOverride(7, 'paged', null), 'paged');
			assert.equal(store.readModeOverride(7), 'paged');
			assert.equal(store.writeDirOverride(7, 'ltr', null), 'ltr');
			assert.equal(store.readDirOverride(7), 'ltr');
		});
	});
});

test('the legacy "vertical" mode value reads as continuous', async () => {
	await withVite(async (vite) => {
		const store: OverrideStore = await vite.ssrLoadModule('/src/pages/reader/overrideStore.ts');
		withStorage((values) => {
			values.set('sumidoku.reader.mode.7', 'vertical');
			assert.equal(store.readModeOverride(7), 'continuous');
			values.set('sumidoku.reader.mode.7', 'diagonal');
			assert.equal(store.readModeOverride(7), null);
		});
	});
});

test('clearing a manga drops all its override keys and nothing else', async () => {
	await withVite(async (vite) => {
		const store: OverrideStore = await vite.ssrLoadModule('/src/pages/reader/overrideStore.ts');
		withStorage((values) => {
			store.writeModeOverride(7, 'webtoon', null);
			store.writeLayoutOverride(7, 'double', 'single');
			store.writeSplitOverride(7, true, false);
			// Legacy globalized keys the manga may still carry
			values.set('sumidoku.reader.bg.7', 'light');
			values.set('sumidoku.reader.anim.7', '0');
			// Another manga's keys and the global defaults stay
			store.writeModeOverride(8, 'continuous', null);
			values.set('sumidoku.reader.bg', 'gray');
			store.clearReaderOverrides(7);
			assert.deepEqual([...values.keys()].sort(), ['sumidoku.reader.bg', 'sumidoku.reader.mode.8']);
		});
	});
});

test('the globalized sweep drops personal-preference overrides only', async () => {
	await withVite(async (vite) => {
		const store: OverrideStore = await vite.ssrLoadModule('/src/pages/reader/overrideStore.ts');
		withStorage((values) => {
			values.set('sumidoku.reader.bg.7', 'light');
			values.set('sumidoku.reader.tapzones.8', 'kindle');
			values.set('sumidoku.reader.anim.9', '0');
			values.set('sumidoku.reader.transition.9', '0');
			// Content overrides and global keys are not swept
			values.set('sumidoku.reader.mode.7', 'webtoon');
			values.set('sumidoku.reader.scale.8', 'width');
			values.set('sumidoku.reader.bg', 'dark');
			values.set('sumidoku.reader.tapzones', 'lr');
			values.set('sumidoku.library.filters', '{}');
			store.sweepGlobalizedOverrides();
			assert.deepEqual(
				[...values.keys()].sort(),
				[
					'sumidoku.library.filters',
					'sumidoku.reader.bg',
					'sumidoku.reader.mode.7',
					'sumidoku.reader.scale.8',
					'sumidoku.reader.tapzones'
				]
			);
		});
	});
});

function source(path: string): string {
	// A Windows checkout hands back CRLF; normalize before matching multi-line needles
	return readFileSync(new URL(path, import.meta.url), 'utf8').replace(/\r\n/g, '\n');
}

test('personal preferences write the global key even with a manga in scope', () => {
	const page = source('../src/pages/ReaderPage.tsx');
	for (const [setter, key] of [
		['setTapZones', 'sumidoku.reader.tapzones'],
		['setBackground', 'sumidoku.reader.bg'],
		['setAnim', 'sumidoku.reader.anim'],
		['setTransition', 'sumidoku.reader.transition']
	] as const) {
		const body = page.slice(page.indexOf(`function ${setter}`));
		assert.match(body, new RegExp(`localStorage\\.(setItem|removeItem)\\('${key}'`));
	}
	// No per-manga keys remain for the globalized settings, and the reader sweeps their leftovers
	assert.doesNotMatch(page, /reader\.(bg|tapzones|anim|transition)\.\$\{/);
	assert.match(page, /useEffect\(\(\) => sweepGlobalizedOverrides\(\), \[\]\)/);
});

test('leaving the library clears the manga’s reader overrides', () => {
	const hooks = source('../src/api/hooks.ts');
	const remove = hooks.slice(hooks.indexOf('useRemoveFromLibrary'));
	assert.match(remove, /clearReaderOverrides\(mangaId\)/);
});
