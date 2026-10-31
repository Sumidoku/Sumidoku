// Import the bare i18next singleton rather than ../i18n: that module wires catalogs via
// Vite's import.meta.glob and reads navigator, neither of which exists under node --test
import i18n from 'i18next';

// date_uploaded comes from the source; both timestamps (seconds/milliseconds) and date strings occur, so normalize everything to milliseconds
export function toMillis(v: number | string | null | undefined): number | null {
	if (v === null || v === undefined) return null;
	if (typeof v === 'number') {
		if (!Number.isFinite(v) || v <= 0) return null;
		return v < 1e12 ? v * 1000 : v;
	}
	const t = Date.parse(v);
	return Number.isNaN(t) ? null : t;
}

export function formatRangeBound(value: number | null | undefined): string {
	if (value === null || value === undefined) return '';
	return String(value);
}

export function formatDate(v: number | string | null | undefined): string {
	const ms = toMillis(v);
	if (ms === null) return '';
	const d = new Date(ms);
	return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(
		d.getDate()
	).padStart(2, '0')}`;
}

export function formatBytes(value: number): string {
	if (value < 1024) return `${value} B`;
	const units = ['KiB', 'MiB', 'GiB', 'TiB'];
	let size = value / 1024;
	let unit = 0;
	while (size >= 1024 && unit < units.length - 1) {
		size /= 1024;
		unit++;
	}
	return `${size.toLocaleString(undefined, { maximumFractionDigits: 1 })} ${units[unit]}`;
}

export function relativeTime(v: number | string | null | undefined): string {
	const ms = toMillis(v);
	if (ms === null) return '';
	const diff = Date.now() - ms;
	const min = Math.floor(diff / 60000);
	if (min < 1) return i18n.t('format.justNow');
	if (min < 60) return i18n.t('format.minutesAgo', { count: min });
	const hour = Math.floor(min / 60);
	if (hour < 24) return i18n.t('format.hoursAgo', { count: hour });
	const day = Math.floor(hour / 24);
	if (day < 30) return i18n.t('format.daysAgo', { count: day });
	return formatDate(ms);
}

const STATUS_KEYS: Record<string, string> = {
	ongoing: 'format.status.ongoing',
	completed: 'format.status.completed',
	hiatus: 'format.status.hiatus',
	cancelled: 'format.status.cancelled',
	unknown: 'format.status.unknown'
};

export function statusText(s: string | null | undefined): string {
	if (!s) return i18n.t('format.status.unknown');
	const key = STATUS_KEYS[s.toLowerCase()];
	return key ? i18n.t(key) : s;
}

export function entrySubtitle(m: { authors?: Array<string> | null; status?: string | null }): string | undefined {
	const author = m.authors?.[0];
	if (author) return author;
	if (!m.status || m.status.toLowerCase() === 'unknown') return undefined;
	return statusText(m.status);
}

/**
 * A chapter number counts as "set" only when it is a finite, non-zero number.
 * Sources commonly leave chapterNumber null; some also use 0 as a sentinel for
 * "no number" (matching the Suwayomi export mapping in crates/server/src/api/
 * suwayomi_backup.rs: `(chapter_number != 0.0).then_some(...)`). NaN cannot
 * survive JSON transport, but guard against it anyway.
 */
export function hasChapterNumber(n: number | null | undefined): boolean {
	return n !== null && n !== undefined && Number.isFinite(n) && n !== 0;
}

export function chapterLabel(c: {
	volume_number: number | null;
	chapter_number: number | null;
	title: string | null;
}): string {
	const vol =
		c.volume_number !== null && c.volume_number !== undefined
			? i18n.t('format.volumeNumber', { number: c.volume_number })
			: '';
	const num = hasChapterNumber(c.chapter_number)
		? i18n.t('format.chapterNumber', { number: c.chapter_number })
		: '';
	const title = c.title?.trim() ?? '';
	const parts = [vol, num].filter(Boolean);
	if (parts.length > 0 && title) return `${parts.join(' · ')} · ${title}`;
	return parts.join(' · ') || title || i18n.t('format.unnamedChapter');
}
