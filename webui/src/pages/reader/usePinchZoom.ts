import { useCallback, useEffect, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { PinchZoomController } from './pinchZoom';

interface PinchZoomOptions {
	zoomedTouchAction: 'none' | 'pan-y';
}

export function usePinchZoom({ zoomedTouchAction }: PinchZoomOptions) {
	const targetRef = useRef<HTMLDivElement>(null);
	const [zoomed, setZoomed] = useState(false);
	const zoomedRef = useRef(false);
	const activePointers = useRef(new Set<number>());
	const controllerRef = useRef<PinchZoomController | null>(null);
	if (controllerRef.current === null) {
		controllerRef.current = new PinchZoomController(({ s, tx, ty }) => {
			const el = targetRef.current;
			if (el) el.style.transform = s === 1 ? '' : `translate(${tx}px, ${ty}px) scale(${s})`;
			zoomedRef.current = s > 1;
			setZoomed(s > 1);
		});
	}
	const controller = controllerRef.current;

	const onPointerDown = useCallback((e: ReactPointerEvent<HTMLDivElement>) => {
		if (zoomedRef.current || activePointers.current.size > 0) {
			for (const id of activePointers.current) e.currentTarget.setPointerCapture(id);
			e.currentTarget.setPointerCapture(e.pointerId);
		}
		activePointers.current.add(e.pointerId);
		controller.down(e.pointerId, e.clientX, e.clientY, e.currentTarget.getBoundingClientRect());
	}, [controller]);
	const onPointerMove = useCallback((e: ReactPointerEvent<HTMLDivElement>) => {
		controller.move(e.pointerId, e.clientX, e.clientY);
	}, [controller]);
	const finishPointer = useCallback(
		(pointerId: number) => {
			activePointers.current.delete(pointerId);
			controller.up(pointerId);
		},
		[controller]
	);
	const endPointer = useCallback((e: ReactPointerEvent<HTMLDivElement>) => finishPointer(e.pointerId), [finishPointer]);
	useEffect(() => {
		const end = (e: PointerEvent) => {
			if (activePointers.current.has(e.pointerId)) finishPointer(e.pointerId);
		};
		window.addEventListener('pointerup', end);
		window.addEventListener('pointercancel', end);
		return () => {
			window.removeEventListener('pointerup', end);
			window.removeEventListener('pointercancel', end);
		};
	}, [finishPointer]);
	const gestureConsumed = useCallback(() => controller.gestureConsumed(), [controller]);
	const consumeClick = useCallback(() => controller.consumeClick(), [controller]);
	const reset = useCallback(() => {
		activePointers.current.clear();
		controller.reset();
	}, [controller]);

	return {
		targetRef,
		zoomed,
		touchAction: zoomed ? zoomedTouchAction : ('pan-x pan-y' as const),
		onPointerDown,
		onPointerMove,
		onPointerUp: endPointer,
		onPointerCancel: endPointer,
		gestureConsumed,
		consumeClick,
		reset
	};
}
