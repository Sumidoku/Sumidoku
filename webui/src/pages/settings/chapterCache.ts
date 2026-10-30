/** Whole MiB input; an empty or unsafe value must not silently disable the cache. */
export function chapterCacheLimitBytes(raw: string): number | null {
	if (!raw.trim()) return null;
	const value = Number(raw);
	const bytes = value * 1024 ** 2;
	return Number.isSafeInteger(value) && value >= 0 && Number.isSafeInteger(bytes) ? bytes : null;
}
