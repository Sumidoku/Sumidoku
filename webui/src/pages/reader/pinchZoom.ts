export interface ZoomTransform {
	s: number;
	tx: number;
	ty: number;
}

interface Point {
	x: number;
	y: number;
}

interface TargetRect {
	x: number;
	y: number;
	width: number;
	height: number;
}

type GestureMode = 'idle' | 'pinch' | 'pan';

const MAX_SCALE = 5;
const PAN_THRESHOLD = 8;
const SNAP_BACK = 1.08;

export class PinchZoomController {
	private readonly onTransform: (transform: ZoomTransform) => void;
	private readonly pointers = new Map<number, Point>();
	private mode: GestureMode = 'idle';
	private prevDist = 0;
	private prevMid: Point = { x: 0, y: 0 };
	private last: Point = { x: 0, y: 0 };
	private panTravel = 0;
	private transform: ZoomTransform = { s: 1, tx: 0, ty: 0 };
	private origin: Point = { x: 0, y: 0 };
	private bounds = { width: 0, height: 0 };
	private dirty = false;
	private suppressClick = false;

	constructor(onTransform: (transform: ZoomTransform) => void) {
		this.onTransform = onTransform;
	}

	down(pointerId: number, clientX: number, clientY: number, rect: TargetRect) {
		if (this.pointers.size === 0) {
			this.origin = { x: rect.x, y: rect.y };
			this.bounds = { width: rect.width, height: rect.height };
			this.mode = this.transform.s > 1 ? 'pan' : 'idle';
			this.last = { x: clientX - rect.x, y: clientY - rect.y };
			this.panTravel = 0;
			this.dirty = false;
			this.suppressClick = false;
		} else if (this.pointers.size === 1) {
			this.mode = 'pinch';
			this.dirty = true;
			this.prevDist = 0;
		}
		this.pointers.set(pointerId, { x: clientX - this.origin.x, y: clientY - this.origin.y });
	}

	move(pointerId: number, clientX: number, clientY: number) {
		const pointer = this.pointers.get(pointerId);
		if (!pointer) return;
		pointer.x = clientX - this.origin.x;
		pointer.y = clientY - this.origin.y;
		if (this.mode === 'pinch') {
			const [a, b] = [...this.pointers.values()];
			if (!a || !b) return;
			const dist = Math.hypot(a.x - b.x, a.y - b.y);
			const mid = { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
			if (this.prevDist > 0 && dist > 0) {
				const t = this.transform;
				const s = Math.min(MAX_SCALE, Math.max(1, t.s * (dist / this.prevDist)));
				const k = s / t.s;
				const tx = mid.x - (mid.x - t.tx - (mid.x - this.prevMid.x)) * k;
				const ty = mid.y - (mid.y - t.ty - (mid.y - this.prevMid.y)) * k;
				this.apply(s, tx, ty);
			}
			this.prevDist = dist;
			this.prevMid = mid;
		} else if (this.mode === 'pan') {
			const dx = pointer.x - this.last.x;
			const dy = pointer.y - this.last.y;
			this.last = { x: pointer.x, y: pointer.y };
			this.panTravel += Math.abs(dx) + Math.abs(dy);
			const t = this.transform;
			this.apply(t.s, t.tx + dx, t.ty + dy);
		}
	}

	up(pointerId: number) {
		if (!this.pointers.delete(pointerId)) return;
		if (this.pointers.size >= 2) {
			this.prevDist = 0;
			return;
		}
		if (this.pointers.size === 1) {
			const [remaining] = this.pointers.values();
			this.last = { x: remaining.x, y: remaining.y };
			this.prevDist = 0;
			this.mode = this.transform.s > 1 ? 'pan' : 'idle';
			return;
		}
		const t = this.transform;
		if (t.s > 1 && t.s < SNAP_BACK) this.apply(1, 0, 0);
		if (this.dirty || this.panTravel > PAN_THRESHOLD) this.suppressClick = true;
		this.mode = 'idle';
	}

	reset() {
		this.pointers.clear();
		this.mode = 'idle';
		this.prevDist = 0;
		this.panTravel = 0;
		this.dirty = false;
		this.suppressClick = false;
		this.transform = { s: 1, tx: 0, ty: 0 };
		this.onTransform({ ...this.transform });
	}

	gestureConsumed(): boolean {
		return this.dirty || this.panTravel > PAN_THRESHOLD || this.transform.s > 1;
	}

	consumeClick(): boolean {
		const consumed = this.suppressClick;
		this.suppressClick = false;
		return consumed;
	}

	private apply(s: number, tx: number, ty: number) {
		const { width, height } = this.bounds;
		this.transform = {
			s,
			tx: Math.min(0, Math.max(width * (1 - s), tx)),
			ty: Math.min(0, Math.max(height * (1 - s), ty))
		};
		this.onTransform({ ...this.transform });
	}
}
