import { pushToast } from '../components/Toast';
import { translateApiError } from '../i18n';
import type { StreamTerminal } from './protocol';

const BASE = '/api/v1';

/** Unified 401 handling: jump to the login page; login/password-change requests are handled locally by the page (avoids loops and false kicks) */
function handleUnauthorized(path: string) {
	if (location.pathname === '/login' || path === '/auth/login' || path === '/auth/password') return;
	location.assign('/login');
}

export class ApiError extends Error {
	status: number;
	code: string;

	constructor(status: number, code: string, message: string) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.code = code;
	}
}

export interface ApiOptions {
	method?: string;
	body?: unknown;
	/** multipart/form-data upload; the browser sets the Content-Type (with boundary) itself */
	formData?: FormData;
	/** Probing requests (e.g. a 501 meaning listings are unsupported) don't pop an error toast */
	silent?: boolean;
}

export async function api<T>(path: string, opts: ApiOptions = {}): Promise<T> {
	const { method = 'GET', body, formData, silent = false } = opts;
	// Binary uploads (.aib) skip JSON encoding
	const raw = body instanceof Blob || body instanceof ArrayBuffer;
	let res: Response;
	try {
		res = await fetch(BASE + path, {
			method,
			headers:
				formData !== undefined
					? undefined
					: body !== undefined
						? { 'Content-Type': raw ? 'application/octet-stream' : 'application/json' }
						: undefined,
			body: formData ?? (body !== undefined ? (raw ? (body as BodyInit) : JSON.stringify(body)) : undefined)
		});
	} catch {
		const err = new ApiError(0, 'network', '网络请求失败');
		if (!silent) pushToast(translateApiError(err.code, err.message));
		throw err;
	}
	if (!res.ok) {
		let code = 'http';
		let message = `请求失败（HTTP ${res.status}）`;
		try {
			const data = await res.json();
			if (data?.error?.code) code = String(data.error.code);
			if (data?.error?.message) message = String(data.error.message);
		} catch {
			// Keep the default message when the error body is not JSON
		}
		const err = new ApiError(res.status, code, message);
		if (res.status === 401) handleUnauthorized(path);
		if (!silent) pushToast(translateApiError(code, message, { status: res.status }));
		throw err;
	}
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
}

/** v is a version marker (pass manga.cover): after a cover change the endpoint URL is unchanged while the response is cacheable with max-age=86400 — without it the browser keeps showing the old image */
export const coverUrl = (mangaId: number, v?: string | null) => {
	const base = `${BASE}/manga/${mangaId}/cover`;
	return v ? `${base}?v=${encodeURIComponent(v)}` : base;
};

export interface ApiStreamOptions<TPartial extends { type: string }> {
	method?: string;
	body?: unknown;
	silent?: boolean;
	signal?: AbortSignal;
	/** Called for each partial event as it arrives (in the server's send order) */
	onEvent: (event: TPartial) => void;
}

/**
 * Streaming request: Accept: application/x-ndjson is an explicit opt-in, partial events are called back
 * line by line, and the promise resolves with the terminal payload (same shape as a non-streaming response
 * body). When the server does not support streaming (content-type is not ndjson), falls back to whole-body
 * JSON, behaving exactly like api()
 */
export async function apiStream<TFinal, TPartial extends { type: string }>(
	path: string,
	opts: ApiStreamOptions<TPartial>
): Promise<TFinal> {
	const { method = 'GET', body, silent = false, signal, onEvent } = opts;
	let res: Response;
	try {
		res = await fetch(BASE + path, {
			method,
			headers: {
				Accept: 'application/x-ndjson',
				...(body !== undefined ? { 'Content-Type': 'application/json' } : {})
			},
			body: body !== undefined ? JSON.stringify(body) : undefined,
			signal
		});
	} catch (e) {
		if (e instanceof DOMException && e.name === 'AbortError') throw e;
		const err = new ApiError(0, 'network', '网络请求失败');
		if (!silent) pushToast(translateApiError(err.code, err.message));
		throw err;
	}
	if (!res.ok) {
		let code = 'http';
		let message = `请求失败（HTTP ${res.status}）`;
		try {
			const data = await res.json();
			if (data?.error?.code) code = String(data.error.code);
			if (data?.error?.message) message = String(data.error.message);
		} catch {
			// Keep the default message when the error body is not JSON
		}
		const err = new ApiError(res.status, code, message);
		if (res.status === 401) handleUnauthorized(path);
		if (!silent) pushToast(translateApiError(code, message, { status: res.status }));
		throw err;
	}
	const contentType = res.headers.get('content-type') ?? '';
	if (!res.body || !contentType.includes('application/x-ndjson')) {
		return (await res.json()) as TFinal;
	}

	const reader = res.body.getReader();
	const decoder = new TextDecoder();
	let buffer = '';
	let finalPayload: TFinal | undefined;
	let streamError: { code: string; message: string } | null = null;
	for (;;) {
		const { done, value } = await reader.read();
		if (done) break;
		buffer += decoder.decode(value, { stream: true });
		let nl = buffer.indexOf('\n');
		while (nl >= 0) {
			const line = buffer.slice(0, nl).trim();
			buffer = buffer.slice(nl + 1);
			nl = buffer.indexOf('\n');
			if (!line) continue;
			let event: TPartial | StreamTerminal<TFinal>;
			try {
				event = JSON.parse(line) as TPartial | StreamTerminal<TFinal>;
			} catch {
				throw new ApiError(res.status, 'stream_parse', '流式响应包含无法解析的事件');
			}
			if (event.type === 'final') {
				finalPayload = (event as { payload: TFinal }).payload;
			} else if (event.type === 'error') {
				streamError = (event as { error: { code: string; message: string } }).error;
			} else {
				onEvent(event as TPartial);
			}
		}
	}
	if (streamError) {
		const err = new ApiError(res.status, streamError.code, streamError.message);
		if (!silent) pushToast(translateApiError(err.code, err.message));
		throw err;
	}
	if (finalPayload === undefined) {
		const err = new ApiError(res.status, 'stream_terminal', '流式响应缺少终态事件');
		if (!silent) pushToast(translateApiError(err.code, err.message));
		throw err;
	}
	return finalPayload;
}

export const pageUrl = (chapterId: number, index: number, width?: number) =>
	`${BASE}/chapter/${chapterId}/page/${index}${width ? `?width=${width}` : ''}`;

/** CBZ file URL of a downloaded chapter; pair with `<a download>` to trigger a browser download */
export const chapterFileUrl = (chapterId: number) => `${BASE}/chapter/${chapterId}/file`;

/** Full backup download URL; pair with `<a download>` to make the browser save it */
export const backupUrl = () => `${BASE}/backups/document`;

export const sourceIconUrl = (sourceId: number) => `${BASE}/sources/${sourceId}/icon`;

/** Only the server can attach the anti-hotlink headers a source image host declares, so browse/home covers are fetched through the server proxy */
export const sourceImageUrl = (sourceId: number, url: string) =>
	`${BASE}/sources/${sourceId}/image?url=${encodeURIComponent(url)}`;
