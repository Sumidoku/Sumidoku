import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const app = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');

function escapeRegex(value: string) {
	return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

test('every route declares a document title fallback', () => {
	assert.match(app, /element: <DocumentTitleProvider \/>/);
	const routes = [
		['/', 'common.nav.library'],
		['/library', 'common.nav.library'],
		['/browse', 'common.nav.browse'],
		['/browse/:sourceId', 'sourceBrowse.sourceFallback'],
		['/browse/:sourceId/home', 'sourceHome.sourceFallback'],
		['/search', 'common.nav.search'],
		['/manga/:id', 'mangaDetail.documentTitle'],
		['/migrate', 'migrate.title'],
		['/history', 'common.nav.history'],
		['/downloads', 'common.nav.downloads'],
		['/updates', 'common.nav.updates'],
		['/settings', 'common.nav.settings'],
		['*', 'notFound.title'],
		['/reader/:chapterId', 'reader.documentTitleEmpty'],
		['/login', 'login.submit']
	];
	for (const [path, title] of routes) {
		const pattern = new RegExp(`path: '${escapeRegex(path)}', element: [^\\n]*handle: \\{ title: '${escapeRegex(title)}' \\}`);
		assert.match(app, pattern, `missing document title for ${path}`);
	}
});

test('content pages override route titles through the shared hook', () => {
	for (const page of ['MangaDetailPage', 'SourceBrowsePage', 'SourceHomePage', 'ReaderPage']) {
		const source = readFileSync(new URL(`../src/pages/${page}.tsx`, import.meta.url), 'utf8');
		assert.match(source, /useDocumentTitle\(/, `${page} does not set its content title`);
	}
});

test('blank content titles keep the route fallback', () => {
	const source = readFileSync(new URL('../src/components/DocumentTitle.tsx', import.meta.url), 'utf8');
	assert.match(source, /title: title \|\| null/);
});
