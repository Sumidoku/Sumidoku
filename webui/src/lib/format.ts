import { get } from 'svelte/store';
import { _ } from 'svelte-i18n';

/** 非组件环境里的翻译取值（i18n 在根布局已 await 初始化） */
function t(key: string, values?: Record<string, string | number>): string {
	try {
		return get(_)(key, values ? { values } : undefined) as string;
	} catch {
		return key;
	}
}

// date_uploaded 来自源端，时间戳（秒/毫秒）与日期字符串都出现过，统一归一到毫秒
export function toMillis(v: number | string | null | undefined): number | null {
	if (v === null || v === undefined) return null;
	if (typeof v === 'number') {
		if (!Number.isFinite(v) || v <= 0) return null;
		return v < 1e12 ? v * 1000 : v;
	}
	const parsed = Date.parse(v);
	return Number.isNaN(parsed) ? null : parsed;
}

export function formatDate(v: number | string | null | undefined): string {
	const ms = toMillis(v);
	if (ms === null) return '';
	const d = new Date(ms);
	return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(
		d.getDate()
	).padStart(2, '0')}`;
}

export function relativeTime(v: number | string | null | undefined): string {
	const ms = toMillis(v);
	if (ms === null) return '';
	const diff = Date.now() - ms;
	const min = Math.floor(diff / 60000);
	if (min < 1) return t('format.justNow');
	if (min < 60) return t('format.minutesAgo', { n: min });
	const hour = Math.floor(min / 60);
	if (hour < 24) return t('format.hoursAgo', { n: hour });
	const day = Math.floor(hour / 24);
	if (day < 30) return t('format.daysAgo', { n: day });
	return formatDate(ms);
}

const STATUS_KEYS = new Set(['ongoing', 'completed', 'hiatus', 'cancelled', 'unknown']);

export function statusText(s: string | null | undefined): string {
	if (!s) return t('status.unknown');
	const key = s.toLowerCase();
	return STATUS_KEYS.has(key) ? t(`status.${key}`) : s;
}

export function chapterLabel(c: { chapter_number: number | null; title: string | null }): string {
	const num =
		c.chapter_number !== null && c.chapter_number !== undefined
			? t('chapter.number', { n: c.chapter_number })
			: '';
	const title = c.title?.trim() ?? '';
	if (num && title) return `${num} · ${title}`;
	return num || title || t('chapter.untitled');
}
