/**
 * Damped pull-to-next-chapter for the reader's edge gestures (continuous,
 * webtoon and paged). At the chapter end, further forward input accumulates a
 * raw pull distance; the visible offset is that distance compressed by the
 * rubber() curve, so the gesture feels increasingly resistant and the content
 * never flies off screen. Reversing shrinks the raw distance back — sliding
 * all the way back is the cancel, there is no post-jump undo. A touch pull
 * commits when the finger lifts past the threshold; a wheel pull commits on
 * the crossing event itself, and only counts after the wheel has gone quiet
 * once at the edge, so fling inertia can never be read as an intentional pull.
 */
export const PULL_THRESHOLD = 160;
export const RUBBER_MAX = 240;
export const WHEEL_ARM_QUIET_MS = 250;

/** Rubber-band compression: 1:1 at the origin, decaying slope, bounded total. */
export function rubber(raw: number): number {
	return (RUBBER_MAX * raw) / (RUBBER_MAX + raw * 0.9);
}

export type PullState = 'idle' | 'pulling' | 'firing';

export interface PullGestureHooks {
	setTimeout: (cb: () => void, ms: number) => unknown;
	clearTimeout: (handle: unknown) => void;
	/** Live progress for the visual offset; fired on every quantized change. */
	onPull: (offset: number, raw: number, armed: boolean) => void;
	/** The pull committed: touch release or wheel crossing, threshold reached. */
	onTrigger: () => void;
	/** The pull was abandoned (reverse slide, leaving the edge, external gate). */
	onCancel: (hadPull: boolean) => void;
}

export class PullGestureController {
	private raw = 0;
	private wheelArmed = false;
	private wheelTimer: unknown;
	private state: PullState = 'idle';

	constructor(private readonly hooks: PullGestureHooks) {}

	get rawPull(): number {
		return this.raw;
	}

	get pullState(): PullState {
		return this.state;
	}

	private add(d: number): void {
		if (this.state === 'firing') return;
		const next = Math.max(0, this.raw + d);
		if (next === this.raw) return;
		this.raw = next;
		this.state = this.raw > 0 ? 'pulling' : 'idle';
		this.hooks.onPull(rubber(this.raw), this.raw, this.raw >= PULL_THRESHOLD);
	}

	private static normalize(deltaY: number, deltaMode: number, viewportHeight: number): number {
		if (deltaMode === 1) return deltaY * 16; // lines
		if (deltaMode === 2) return deltaY * viewportHeight; // pages
		return deltaY;
	}

	/**
	 * One wheel event. atEdge = the chapter end is reached (hard bottom in the
	 * scroll modes, last spread in paged). Positive deltas pull forward;
	 * negative deltas shrink an ongoing pull.
	 */
	feedWheel(deltaY: number, deltaMode: number, viewportHeight: number, atEdge: boolean): void {
		if (this.state === 'firing') return;
		if (deltaY > 0) {
			if (!atEdge && this.raw === 0) {
				// Scrolling toward the end from mid-chapter is a new approach: its
				// fling must go quiet at the edge again before it can pull
				this.wheelArmed = false;
				this.clearWheelTimer();
				return;
			}
			this.armWheelQuiet();
			// Inertia from the fling that reached the edge is not a pull
			if (!this.wheelArmed) return;
			this.add(PullGestureController.normalize(deltaY, deltaMode, viewportHeight));
			if (this.raw >= PULL_THRESHOLD) this.fire();
		} else if (this.raw > 0) {
			this.add(PullGestureController.normalize(deltaY, deltaMode, viewportHeight));
		}
	}

	/**
	 * One touch/pointer drag delta in the forward direction (positive = pulling
	 * toward the next chapter). Returns true when the caller should take the
	 * gesture over (preventDefault), i.e. the pull owns this pointer now.
	 */
	feedTouch(d: number, atEdge: boolean): boolean {
		if (this.state === 'firing') return false;
		if (d > 0 && !atEdge && this.raw === 0) return false;
		if (this.raw === 0 && d <= 0) return false;
		this.add(d);
		return true;
	}

	/** Pointer lifted: an armed pull commits, anything else springs back. */
	releaseTouch(): void {
		if (this.state !== 'pulling') return;
		if (this.raw >= PULL_THRESHOLD) this.fire();
		else this.cancel();
	}

	private fire(): void {
		this.state = 'firing';
		this.clearWheelTimer();
		this.hooks.onTrigger();
	}

	cancel(): void {
		const had = this.raw > 0;
		this.raw = 0;
		this.state = 'idle';
		this.wheelArmed = false;
		this.clearWheelTimer();
		this.hooks.onCancel(had);
	}

	reset(): void {
		this.cancel();
	}

	dispose(): void {
		this.clearWheelTimer();
	}

	private armWheelQuiet(): void {
		this.clearWheelTimer();
		this.wheelTimer = this.hooks.setTimeout(() => {
			this.wheelTimer = undefined;
			this.wheelArmed = true;
		}, WHEEL_ARM_QUIET_MS);
	}

	private clearWheelTimer(): void {
		if (this.wheelTimer !== undefined) this.hooks.clearTimeout(this.wheelTimer);
		this.wheelTimer = undefined;
	}
}
