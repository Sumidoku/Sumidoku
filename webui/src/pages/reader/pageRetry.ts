// Page-image load retry: one automatic retry after the first error, then the
// failure latches and PageImage shows a manual retry button. The state is keyed
// by src so reusing the component for another page (chapter turns keep the same
// page-index keys) never inherits a stale failure or attempt count.

/** Errors retried automatically before the failure latches */
export const PAGE_AUTO_RETRIES = 1;

export interface PageImageState {
	src: string;
	attempt: number;
	failed: boolean;
}

export function initialImageState(src: string): PageImageState {
	return { src, attempt: 0, failed: false };
}

/** An <img> error: auto-retry while attempts remain, otherwise latch the failure */
export function imageErrored(prev: PageImageState | null, src: string): PageImageState {
	const cur = prev !== null && prev.src === src ? prev : initialImageState(src);
	return cur.attempt < PAGE_AUTO_RETRIES
		? { src, attempt: cur.attempt + 1, failed: false }
		: { src, attempt: cur.attempt, failed: true };
}

/** The failure placeholder's retry button reloads with a fresh cache-buster */
export function imageRetried(prev: PageImageState): PageImageState {
	return { src: prev.src, attempt: prev.attempt + 1, failed: false };
}

/** The effective state for a render: a stored state belongs only to its own src */
export function imageStateFor(state: PageImageState | null, src: string): { attempt: number; failed: boolean } {
	return state !== null && state.src === src
		? { attempt: state.attempt, failed: state.failed }
		: { attempt: 0, failed: false };
}

/** Attempts past the first carry a cache-buster so a cached error response is not
    replayed; the separator must match whether the base URL already has a query
    (a bare page URL takes `?`, one with `?revision=` takes `&`) */
export function retrySrc(src: string, attempt: number): string {
	if (attempt <= 0) return src;
	return `${src}${src.includes('?') ? '&' : '?'}r=${attempt}`;
}

/** Whether the page is still loading: only the exact effective URL whose load
    event fired clears the indicator, so an automatic retry (a new cache-buster)
    flips it back on, and a load recorded for another page never leaks in */
export function imageLoading(loadedSrc: string | null, src: string, attempt: number): boolean {
	return loadedSrc !== retrySrc(src, attempt);
}
