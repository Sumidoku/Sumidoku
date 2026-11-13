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

interface Probe {
	triggers: number[];
	cancels: boolean[];
	pulls: [number, number, boolean][];
}

async function makeCtl(
	vite: ViteDevServer,
	timers: FakeTimers
): Promise<{ ctl: import('../src/pages/reader/pullNextChapter').PullGestureController; probe: Probe; rubber: (raw: number) => number; threshold: number }> {
	const mod = await vite.ssrLoadModule('/src/pages/reader/pullNextChapter.ts');
	const probe: Probe = { triggers: [], cancels: [], pulls: [] };
	const ctl = new mod.PullGestureController({
		...timers.hooks,
		onPull: (offset: number, raw: number, armed: boolean) => probe.pulls.push([offset, raw, armed]),
		onTrigger: () => probe.triggers.push(1),
		onCancel: (had: boolean) => probe.cancels.push(had)
	});
	return { ctl, probe, rubber: mod.rubber, threshold: mod.PULL_THRESHOLD };
}

test('rubber compresses: 1:1 at the origin, monotonic, bounded by the asymptote', async () => {
	await withVite(async (vite) => {
		const { rubber, threshold } = await makeCtl(vite, fakeTimers());
		assert.equal(rubber(0), 0);
		const slopeAtOrigin = (rubber(1) - rubber(0)) / 1;
		assert.ok(slopeAtOrigin > 0.98, `origin slope ≈ 1, got ${slopeAtOrigin}`);
		const slopeAtThreshold = rubber(threshold + 1) - rubber(threshold);
		assert.ok(slopeAtThreshold < 0.56 && slopeAtThreshold > 0.52, `threshold slope ≈ 0.54, got ${slopeAtThreshold}`);
		// Monotonic and forever below the 240/0.9 ≈ 267px asymptote
		for (const raw of [400, 2000, 100000]) {
			assert.ok(rubber(raw) > rubber(raw - 1));
			assert.ok(rubber(raw) < 240 / 0.9);
		}
		// At the threshold the content has travelled ≈70.6px — the commit
		// lands on a drag compressed well below the raw 96px
		assert.ok(rubber(threshold) > 70.4 && rubber(threshold) < 70.8);
	});
});

test('wheel inertia landing at the chapter end is not a pull until the wheel goes quiet', async () => {
	await withVite(async (vite) => {
		const timers = fakeTimers();
		const { ctl, probe } = await makeCtl(vite, timers);
		// A fling arrives at the end and keeps sending large deltas at the edge
		ctl.feedWheel(500, 0, 800, true);
		ctl.feedWheel(500, 0, 800, true);
		assert.deepEqual(probe.triggers, [], 'unarmed wheel deltas never count');
		// The wheel settles; the user's next scroll is the intentional pull
		timers.fireQuiet();
		ctl.feedWheel(60, 0, 800, true);
		assert.deepEqual(probe.triggers, []);
		ctl.feedWheel(40, 0, 800, true);
		assert.deepEqual(probe.triggers, [1], '60+40 crosses the 96px threshold once armed');
		ctl.dispose();
	});
});

test('a later fling toward the end must go quiet at the edge again before it pulls', async () => {
	await withVite(async (vite) => {
		const timers = fakeTimers();
		const { ctl, probe } = await makeCtl(vite, timers);
		// The first visit to the end arms the wheel
		ctl.feedWheel(40, 0, 800, true);
		timers.fireQuiet();
		// The reader scrolls back up, then flings toward the end again
		ctl.feedWheel(-300, 0, 800, false);
		ctl.feedWheel(60, 0, 800, false);
		// That fling's inertia lands at the edge with no quiet gap
		for (let i = 0; i < 6; i++) ctl.feedWheel(40, 0, 800, true);
		assert.deepEqual(probe.triggers, [], 'the earlier arming must not carry over to a new approach');
		assert.deepEqual(probe.pulls, []);
		timers.fireQuiet();
		ctl.feedWheel(60, 0, 800, true);
		ctl.feedWheel(40, 0, 800, true);
		assert.deepEqual(probe.triggers, [1], 'it pulls once the new fling has gone quiet');
		ctl.dispose();
	});
});

test('wheel deltas away from the edge never start a pull', async () => {
	await withVite(async (vite) => {
		const timers = fakeTimers();
		const { ctl, probe } = await makeCtl(vite, timers);
		ctl.feedWheel(0, 0, 800, true);
		timers.fireQuiet();
		ctl.feedWheel(200, 0, 800, false);
		assert.deepEqual(probe.triggers, []);
		assert.deepEqual(probe.pulls, [], 'mid-chapter wheel input is ordinary scrolling');
		ctl.dispose();
	});
});

test('line and page delta modes normalize to pixels', async () => {
	await withVite(async (vite) => {
		const timers = fakeTimers();
		const { ctl, probe } = await makeCtl(vite, timers);
		// A positive edge delta opens the quiet window (it does not count itself)
		ctl.feedWheel(1, 0, 800, true);
		timers.fireQuiet();
		ctl.feedWheel(5, 1, 800, true); // 5 lines × 16px = 80 < 96
		assert.deepEqual(probe.triggers, []);
		ctl.feedWheel(0.1, 2, 800, true); // 0.1 page × 800px = 80 → 160
		assert.deepEqual(probe.triggers, [1]);
		ctl.dispose();
	});
});

test('a touch pull commits on release past the threshold, and a partial pull cancels instead', async () => {
	await withVite(async (vite) => {
		const { ctl, probe } = await makeCtl(vite, fakeTimers());
		// Touch needs no arming: momentum scrolling emits no touch events
		ctl.feedTouch(60, true);
		ctl.feedTouch(30, true);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, [], '90px is below the threshold');
		assert.deepEqual(probe.cancels, [true], 'the partial pull springs back');
		ctl.feedTouch(60, true);
		ctl.feedTouch(40, true);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, [1], 'an armed touch pull commits when the finger lifts');
		ctl.dispose();
	});
});

test('sliding back shrinks the pull, and reversing to zero abandons it', async () => {
	await withVite(async (vite) => {
		const { ctl, probe } = await makeCtl(vite, fakeTimers());
		ctl.feedTouch(80, true);
		assert.equal(ctl.rawPull, 80);
		assert.equal(ctl.feedTouch(-60, true), true, 'the pull still owns the drag while raw > 0');
		assert.equal(ctl.rawPull, 20);
		// Back at the start: that move is still consumed (it cancels the pull),
		// but the raw distance clamps at zero and the next downward drag
		// returns to native scrolling
		assert.equal(ctl.feedTouch(-200, true), true);
		assert.equal(ctl.rawPull, 0);
		assert.equal(ctl.feedTouch(-50, true), false);
		const last = probe.pulls[probe.pulls.length - 1];
		assert.deepEqual(last, [0, 0, false]);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, [], 'nothing left to commit');
		ctl.dispose();
	});
});

test('touch deltas away from the edge neither start nor grow a pull', async () => {
	await withVite(async (vite) => {
		const { ctl, probe } = await makeCtl(vite, fakeTimers());
		assert.equal(ctl.feedTouch(120, false), false, 'mid-chapter drags are ordinary scrolling');
		assert.equal(ctl.rawPull, 0);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, []);
		ctl.dispose();
	});
});

test('firing consumes the gesture: no double trigger, cancel reports and disarms', async () => {
	await withVite(async (vite) => {
		const timers = fakeTimers();
		const { ctl, probe } = await makeCtl(vite, timers);
		ctl.feedTouch(200, true);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, [1]);
		assert.equal(ctl.pullState, 'firing');
		// Input during the slide-off is swallowed
		ctl.feedTouch(300, true);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, [1]);
		// A reset pull must not fire on release, and cancel reports what it had
		ctl.cancel();
		assert.deepEqual(probe.cancels[probe.cancels.length - 1], true);
		ctl.feedTouch(200, true);
		ctl.cancel();
		assert.deepEqual(probe.cancels[probe.cancels.length - 1], true);
		ctl.releaseTouch();
		assert.deepEqual(probe.triggers, [1]);
		ctl.dispose();
	});
});
