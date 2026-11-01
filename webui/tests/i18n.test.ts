import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { createServer } from 'vite';

const LOCALES = ['en', 'de', 'es', 'fr', 'it', 'ja', 'ko', 'ru', 'zh-CN', 'zh-TW'];
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
				const baseKey = key.replace(PLURAL_SUFFIX, '');
				const enTokens = tokens(en[baseKey] ?? en[`${baseKey}_one`] ?? en[`${baseKey}_other`] ?? en[`${baseKey}_plural`] ?? '');
				for (const token of enTokens) assert.ok(tokens(value).includes(token), `${lang}/${file}:${key} missing ${token}`);
			}
		}
	}
});

test('all ten locales are registered and English is the fallback', async () => {
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
		assert.equal(i18n.options.lng, 'en');
		assert.equal(i18n.t('common.save'), 'Save');
		assert.equal(i18n.getFixedT('ru')('settings.groupWithCount', { name: 'Группа', count: 2 }), 'Группа (2 произведения)');
		assert.equal(i18n.getFixedT('ru')('settings.groupWithCount', { name: 'Группа', count: 5 }), 'Группа (5 произведений)');
		assert.equal(i18n.getFixedT('ja')('format.minutesAgo', { count: 1 }), '1分前');
	} finally {
		await vite.close();
	}
});

test('regional browser languages resolve and an explicit language choice persists', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	let loadCount = 0;
	const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
	let storedLanguage = null;
	let savedLanguage = null;
	Object.defineProperty(globalThis, 'localStorage', {
		configurable: true,
		value: {
			getItem: () => storedLanguage,
			setItem: (_key, value) => {
				savedLanguage = value;
			}
		}
	});
	async function loadLanguage(browserLanguage) {
		Object.defineProperty(globalThis, 'navigator', {
			configurable: true,
			value: { language: browserLanguage }
		});
		return (await vite.ssrLoadModule(`/src/i18n?detect=${loadCount++}`)).default;
	}
	try {
		assert.equal((await loadLanguage('de-DE')).language, 'de');
		assert.equal((await loadLanguage('es-MX')).language, 'es');
		assert.equal((await loadLanguage('ja-JP')).language, 'ja');
		assert.equal((await loadLanguage('ko-KR')).language, 'ko');
		assert.equal((await loadLanguage('ru-RU')).language, 'ru');
		assert.equal((await loadLanguage('zh-Hant-HK')).language, 'zh-TW');
		assert.equal((await loadLanguage('zh-Hans-CN')).language, 'zh-CN');
		storedLanguage = 'ko';
		assert.equal((await loadLanguage('en-US')).language, 'ko');
		const selected = await loadLanguage('en-US');
		const { setLanguage } = await vite.ssrLoadModule(`/src/i18n?detect=${loadCount - 1}`);
		setLanguage('zh-TW');
		assert.equal(savedLanguage, 'zh-TW');
		await selected.changeLanguage('ko');
	} finally {
		Reflect.deleteProperty(globalThis, 'localStorage');
		if (originalNavigator) Object.defineProperty(globalThis, 'navigator', originalNavigator);
		else Reflect.deleteProperty(globalThis, 'navigator');
		await vite.close();
	}
});

test('shell language switcher renders as an accessible icon button', async () => {
	const vite = await createServer({
		root: fileURLToPath(new URL('../', import.meta.url)),
		configFile: false,
		logLevel: 'silent',
		appType: 'custom',
		server: { middlewareMode: true }
	});
	try {
		const { default: i18n } = await vite.ssrLoadModule('/src/i18n');
		await i18n.changeLanguage('en');
		const { LanguageMenu } = await vite.ssrLoadModule('/src/layout/LanguageMenu.tsx');
		const markup = renderToStaticMarkup(createElement(LanguageMenu));
		assert.match(markup, /aria-label="Language"/);
		assert.match(markup, /aria-expanded="false"/);
	} finally {
		await vite.close();
	}
});
