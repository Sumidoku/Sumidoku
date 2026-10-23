import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';

test('chapter menu consumes the row click before toggling', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { handleMenuToggle } = await vite.ssrLoadModule('/src/components/MenuButton.tsx');
		const calls: string[] = [];
		handleMenuToggle({ stopPropagation: () => calls.push('stop') }, () => calls.push('toggle'));
		assert.deepEqual(calls, ['stop', 'toggle']);
	} finally {
		await vite.close();
	}
});
