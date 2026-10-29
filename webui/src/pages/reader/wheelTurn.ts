/**
 * Wheel page turning for the paged reader, ported from KMReader's PagedReader.
 * Deltas accumulate until they cross a threshold and emit one page turn, then the
 * controller locks until the wheel goes quiet — trackpads keep sending inertia
 * deltas after the fingers lift, and without the quiet lock one swipe flips many
 * pages.
 */
export const WHEEL_TURN_THRESHOLD = 48;
export const WHEEL_QUIET_MS = 200;

export interface WheelTurnHooks {
	setTimeout: (cb: () => void, ms: number) => unknown;
	clearTimeout: (handle: unknown) => void;
	onTurn: (forward: boolean) => void;
}

export class WheelTurnController {
	private acc = 0;
	private locked = false;
	private quiet: unknown;

	constructor(
		private readonly hooks: WheelTurnHooks,
		private readonly threshold = WHEEL_TURN_THRESHOLD,
		private readonly quietMs = WHEEL_QUIET_MS
	) {}

	/**
	 * Accumulates one wheel event and emits at most one turn per gesture. deltaMode
	 * follows the WheelEvent convention (0 pixels, 1 lines, 2 pages); line and page
	 * deltas are normalized to pixels first. Every fed event is consumed, so the
	 * caller should preventDefault whenever it feeds.
	 */
	feed(deltaY: number, deltaMode: number, viewportHeight: number): void {
		const delta = deltaMode === 1 ? deltaY * 16 : deltaMode === 2 ? deltaY * viewportHeight : deltaY;
		this.armQuietTimer();
		if (this.locked) return;
		this.acc += delta;
		if (Math.abs(this.acc) >= this.threshold) {
			const forward = this.acc > 0;
			this.acc = 0;
			this.locked = true;
			this.hooks.onTurn(forward);
		}
	}

	dispose(): void {
		if (this.quiet !== undefined) this.hooks.clearTimeout(this.quiet);
		this.quiet = undefined;
	}

	private armQuietTimer(): void {
		if (this.quiet !== undefined) this.hooks.clearTimeout(this.quiet);
		this.quiet = this.hooks.setTimeout(() => {
			this.quiet = undefined;
			this.acc = 0;
			this.locked = false;
		}, this.quietMs);
	}
}
