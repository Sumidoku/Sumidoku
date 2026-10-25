import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createServer } from 'vite';

const LOCALES = ['en', 'zh-CN', 'fr', 'it'];
// `_plural` is the legacy v3 suffix still carried by a few keys
const PLURAL_SUFFIX = /_(zero|one|two|few|many|other|plural)$/;

const localesDir = fileURLToPath(new URL('../src/i18n/locales/', import.meta.url));

function flat(obj, prefix = '', out = {}) {
	for (const [k, v] of Object.entries(obj)) {
		const key = prefix ? `${prefix}.${k}` : k;
		if (v && typeof v === 'object') flat(v, key, out);
		else out[key] = String(v);
	}
	return out;
}

function catalog(lang, file) {
	return flat(JSON.parse(readFileSync(join(localesDir, lang, file), 'utf8')));
}

test('locale catalogs cover the English keys modulo plural forms and keep interpolation tokens', () => {
	const files = readdirSync(join(localesDir, 'en')).filter((f) => f.endsWith('.json'));
	const baseKeys = (obj) => [...new Set(Object.keys(obj).map((k) => k.replace(PLURAL_SUFFIX, '')))].sort();
	const tokens = (s) => (s.match(/\{\{[^}]*\}\}/g) || []).sort();
	for (const lang of LOCALES) {
		for (const file of files) {
			const en = catalog('en', file);
			const other = catalog(lang, file);
			assert.deepEqual(baseKeys(other), baseKeys(en), `${lang}/${file}`);
			for (const [key, value] of Object.entries(other)) {
				const enValue = en[key] ?? en[`${key}_other`] ?? '';
				assert.deepEqual(tokens(value), tokens(enValue), `${lang}/${file}:${key}`);
			}
		}
	}
});

test('all four locales are registered and English is the fallback', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const i18n = (await vite.ssrLoadModule('/src/i18n')).default;
		for (const lang of LOCALES) assert.ok(i18n.hasResourceBundle(lang, 'translation'), lang);
		assert.deepEqual(i18n.options.fallbackLng, ['en']);
	} finally {
		await vite.close();
	}
});
