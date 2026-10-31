import type { QueryClient } from '@tanstack/react-query';
import { api } from './client';
import { qk } from './hooks';
import { TRACKER_PROVIDER_NAMES } from './trackers';
import type { TrackerProvider, MetadataRestoreRequest, MetadataRestoreResult } from './types';

export function restoreMetadata(mangaId: number, body: MetadataRestoreRequest = {}) {
	return api<MetadataRestoreResult>(`/manga/${mangaId}/metadata/restore`, { method: 'POST', body });
}

/** Drop every cached read that shows the library entry after it changed */
export function invalidateMangaMetadata(qc: QueryClient, mangaId: number) {
	void qc.invalidateQueries({ queryKey: qk.manga(mangaId) });
	void qc.invalidateQueries({ queryKey: qk.library });
}

/** Human label for a lock provider reference (`tracker:anilist`, `source:<source key>`, `manual`) */
export function lockProviderLabel(provider: string, sourceName: (key: string) => string | undefined): string {
	const tracker = provider.startsWith('tracker:') ? provider.slice('tracker:'.length) : null;
	if (tracker) return TRACKER_PROVIDER_NAMES[tracker as TrackerProvider] ?? tracker;
	// The lock names the source by its natural key: an instance id would not resolve on
	// another install, so the key is what the label looks up
	const sourceKey = provider.startsWith('source:') ? provider.slice('source:'.length) : '';
	if (sourceKey) return sourceName(sourceKey) ?? provider;
	return provider;
}
