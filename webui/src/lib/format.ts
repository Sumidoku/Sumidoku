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
	if (min < 1) return '刚刚';
	if (min < 60) return `${min} 分钟前`;
	const hour = Math.floor(min / 60);
	if (hour < 24) return `${hour} 小时前`;
	const day = Math.floor(hour / 24);
	if (day < 30) return `${day} 天前`;
	return formatDate(ms);
}

const STATUS_TEXT: Record<string, string> = {
	ongoing: '连载中',
	completed: '已完结',
	hiatus: '休刊中',
	cancelled: '已中止',
	unknown: '未知'
};

export function statusText(s: string | null | undefined): string {
	if (!s) return '未知';
	return STATUS_TEXT[s.toLowerCase()] ?? s;
}

export function chapterLabel(c: {
	volume_number: number | null;
	chapter_number: number | null;
	title: string | null;
}): string {
	const vol =
		c.volume_number !== null && c.volume_number !== undefined ? `第 ${c.volume_number} 卷` : '';
	const num =
		c.chapter_number !== null && c.chapter_number !== undefined
			? `第 ${c.chapter_number} 话`
			: '';
	const title = c.title?.trim() ?? '';
	const parts = [vol, num].filter(Boolean);
	if (parts.length > 0 && title) return `${parts.join(' · ')} · ${title}`;
	return parts.join(' · ') || title || '未命名章节';
}
