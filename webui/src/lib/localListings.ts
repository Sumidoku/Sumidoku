// Local-source listing labels. The server sends plain English names so third-party clients can
// show them as-is; the web UI translates by the listing's stable id instead, so a changed English
// label never silently drops the translation. Home sections and the browse segmented control share
// this one table.
import type { TFunction } from 'i18next';

const LOCAL_LISTING_KEYS: Record<string, string> = {
	all: 'sourceHome.all',
	'continue-reading': 'sourceHome.continueReading',
	'latest-updates': 'sourceHome.latestUpdates',
	'latest-added': 'sourceHome.latestAdded',
	'recently-read': 'sourceHome.recentlyRead'
};

/** Translated label for a local listing id, or the fallback when the id is unknown */
export function localListingLabel(id: string, fallback: string, t: TFunction): string {
	const key = LOCAL_LISTING_KEYS[id];
	return key ? t(key) : fallback;
}

/** Home section title: local sections take their label from the section's listing id */
export function homeSectionTitle(
	title: string,
	listingId: string | null,
	isLocal: boolean,
	t: TFunction
): string {
	return isLocal && listingId ? localListingLabel(listingId, title, t) : title;
}
