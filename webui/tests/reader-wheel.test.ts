import assert from 'node:assert/strict';
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

interface FakeTimers {
	hooks: { setTimeout: (cb: () => void, ms: number) => unknown; clearTimeout: (h: unknown) => void };
	fireQuiet: () => void;
}

function fakeTimers(): FakeTimers {
	const pending = new Map<number, () => void>();
	let next = 1;
	return {
		hooks: {
			setTimeout: (cb) => {
				const id = next++;
				pending.set(id, cb);
				return id;
			},
			clearTimeout: (h) => {
				pending.delete(h as number);
			}
		},
		fireQuiet: () => {
			const cbs = [...pending.values()];
			pending.clear();
			cbs.forEach((cb) => cb());
		}
	};
}

test('wheel deltas accumulate across events until they cross the threshold', async () => {
	await withVite(async (vite) => {
		const { WheelTurnController } = await vite.ssrLoadModule('/src/pages/reader/wheelTurn.ts');
		const timers = fakeTimers();
		const turns: boolean[] = [];
		const ctl = new WheelTurnController({ ...timers.hooks, onTurn: (f) => turns.push(f) });
		ctl.feed(20, 0, 800);
		ctl.feed(20, 0, 800);
		assert.deepEqual(turns, []);
		ctl.feed(10, 0, 800);
		assert.deepEqual(turns, [true], '20+20+10 crosses the 48px threshold once');
		ctl.dispose();
	});
});

test('a turn locks the gesture until the wheel goes quiet', async () => {
	await withVite(async (vite) => {
		const { WheelTurnController } = await vite.ssrLoadModule('/src/pages/reader/wheelTurn.ts');
		const timers = fakeTimers();
		const turns: boolean[] = [];
		const ctl = new WheelTurnController({ ...timers.hooks, onTurn: (f) => turns.push(f) });
		ctl.feed(100, 0, 800);
		assert.deepEqual(turns, [true]);
		// Trackpad inertia keeps feeding large deltas; none of them turn again
		ctl.feed(100, 0, 800);
		ctl.feed(100, 0, 800);
		assert.deepEqual(turns, [true]);
		timers.fireQuiet();
		ctl.feed(100, 0, 800);
		assert.deepEqual(turns, [true, true], 'the next gesture turns after the quiet window');
		ctl.dispose();
	});
});

test('line and page delta modes normalize to pixels', async () => {
	await withVite(async (vite) => {
		const { WheelTurnController } = await vite.ssrLoadModule('/src/pages/reader/wheelTurn.ts');
		const timers = fakeTimers();
		const turns: boolean[] = [];
		const ctl = new WheelTurnController({ ...timers.hooks, onTurn: (f) => turns.push(f) });
		ctl.feed(2, 1, 800); // 2 lines × 16px = 32 < 48
		assert.deepEqual(turns, []);
		ctl.feed(1, 1, 800); // +16 = 48
		assert.deepEqual(turns, [true]);
		timers.fireQuiet();
		ctl.feed(0.1, 2, 800); // 0.1 page × 800px viewport = 80
		assert.deepEqual(turns, [true, true]);
		ctl.dispose();
	});
});

test('scrolling up turns backwards and opposing deltas cancel', async () => {
	await withVite(async (vite) => {
		const { WheelTurnController } = await vite.ssrLoadModule('/src/pages/reader/wheelTurn.ts');
		const timers = fakeTimers();
		const turns: boolean[] = [];
		const ctl = new WheelTurnController({ ...timers.hooks, onTurn: (f) => turns.push(f) });
		ctl.feed(30, 0, 800);
		ctl.feed(-30, 0, 800);
		assert.deepEqual(turns, [], 'opposing deltas cancel before the threshold');
		ctl.feed(-60, 0, 800);
		assert.deepEqual(turns, [false]);
		ctl.dispose();
	});
});
