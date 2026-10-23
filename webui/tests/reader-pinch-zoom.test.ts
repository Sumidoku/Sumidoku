import assert from 'node:assert/strict';
import test from 'node:test';
import { PinchZoomController } from '../src/pages/reader/pinchZoom.ts';
import type { ZoomTransform } from '../src/pages/reader/pinchZoom.ts';

const RECT = { x: 0, y: 0, width: 400, height: 200 };

function createHarness() {
	let current: ZoomTransform = { s: 1, tx: 0, ty: 0 };
	const controller = new PinchZoomController((next) => {
		current = next;
	});
	return { controller, current: () => current };
}

function zoomToTwo(controller: PinchZoomController) {
	controller.down(1, 50, 50, RECT);
	controller.down(2, 150, 50, RECT);
	controller.move(1, 50, 50);
	controller.move(2, 250, 50);
	controller.up(2);
	controller.up(1);
}

test('pinch scales around the moving midpoint in target coordinates', () => {
	const { controller, current } = createHarness();
	const rect = { x: 10, y: 20, width: 400, height: 200 };
	controller.down(1, 60, 70, rect);
	controller.down(2, 160, 70, rect);
	controller.move(1, 60, 70);
	controller.move(2, 260, 70);
	assert.deepEqual(current(), { s: 2, tx: -50, ty: -50 });
});

test('one-finger pan follows the pointer and stays within target bounds', () => {
	const { controller, current } = createHarness();
	zoomToTwo(controller);
	controller.down(3, 100, 100, RECT);
	controller.move(3, 80, 80);
	assert.deepEqual(current(), { s: 2, tx: -70, ty: -70 });
	controller.move(3, -300, -300);
	assert.deepEqual(current(), { s: 2, tx: -400, ty: -200 });
	controller.move(3, 300, 300);
	assert.deepEqual(current(), { s: 2, tx: 0, ty: 0 });
});

test('pinch scale is capped at 5', () => {
	const { controller, current } = createHarness();
	controller.down(1, 150, 100, RECT);
	controller.down(2, 250, 100, RECT);
	controller.move(1, 150, 100);
	controller.move(2, 1050, 100);
	assert.equal(current().s, 5);
	controller.move(2, 2050, 100);
	assert.equal(current().s, 5);
});

test('a finished scale below 1.08 snaps back to identity', () => {
	const { controller, current } = createHarness();
	controller.down(1, 190, 100, RECT);
	controller.down(2, 210, 100, RECT);
	controller.move(1, 190, 100);
	controller.move(2, 211, 100);
	assert.ok(Math.abs(current().s - 1.05) < 1e-12);
	controller.up(2);
	assert.ok(Math.abs(current().s - 1.05) < 1e-12);
	controller.up(1);
	assert.deepEqual(current(), { s: 1, tx: 0, ty: 0 });
});

test('pinch and pan gestures suppress one trailing click', () => {
	const { controller } = createHarness();
	assert.equal(controller.gestureConsumed(), false);
	assert.equal(controller.consumeClick(), false);
	controller.down(1, 100, 100, RECT);
	controller.down(2, 120, 100, RECT);
	assert.equal(controller.gestureConsumed(), true);
	controller.up(2);
	controller.up(1);
	assert.equal(controller.gestureConsumed(), true);
	assert.equal(controller.consumeClick(), true);
	assert.equal(controller.consumeClick(), false);
	assert.equal(controller.gestureConsumed(), true);

	zoomToTwo(controller);
	controller.down(3, 100, 100, RECT);
	controller.move(3, 108, 100);
	controller.up(3);
	assert.equal(controller.consumeClick(), false);
	controller.down(4, 100, 100, RECT);
	controller.move(4, 109, 100);
	controller.up(4);
	assert.equal(controller.consumeClick(), true);
	assert.equal(controller.consumeClick(), false);
});

test('reset restores identity and clears active gesture state', () => {
	const { controller, current } = createHarness();
	zoomToTwo(controller);
	controller.down(3, 100, 100, RECT);
	controller.move(3, 130, 100);
	controller.reset();
	assert.deepEqual(current(), { s: 1, tx: 0, ty: 0 });
	assert.equal(controller.gestureConsumed(), false);
	assert.equal(controller.consumeClick(), false);
	controller.move(3, 160, 100);
	controller.up(3);
	assert.deepEqual(current(), { s: 1, tx: 0, ty: 0 });
	controller.down(4, 100, 100, RECT);
	controller.up(4);
	assert.equal(controller.gestureConsumed(), false);
	assert.equal(controller.consumeClick(), false);
});

test('pointer cancellation releases pointers and keeps click suppression', () => {
	const { controller, current } = createHarness();
	controller.down(1, 50, 50, RECT);
	controller.down(2, 150, 50, RECT);
	controller.move(1, 50, 50);
	controller.move(2, 250, 50);
	controller.up(1);
	controller.move(2, 270, 50);
	assert.deepEqual(current(), { s: 2, tx: -30, ty: -50 });
	controller.up(2);
	const finished = current();
	controller.move(1, 100, 100);
	controller.move(2, 300, 100);
	assert.deepEqual(current(), finished);
	assert.equal(controller.consumeClick(), true);
	assert.equal(controller.consumeClick(), false);
});
