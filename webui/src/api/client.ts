import { pushToast } from '../components/Toast';

const BASE = '/api/v1';

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
	/** 探测性请求（如 404 表示不支持 listings）不弹错误提示 */
	silent?: boolean;
}

export async function api<T>(path: string, opts: ApiOptions = {}): Promise<T> {
	const { method = 'GET', body, silent = false } = opts;
	let res: Response;
	try {
		res = await fetch(BASE + path, {
			method,
			headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
			body: body !== undefined ? JSON.stringify(body) : undefined
		});
	} catch {
		const err = new ApiError(0, 'network', '网络请求失败');
		if (!silent) pushToast(err.message);
		throw err;
	}
	if (!res.ok) {
		let code = 'unknown';
		let message = `请求失败（HTTP ${res.status}）`;
		try {
			const data = await res.json();
			if (data?.error?.code) code = String(data.error.code);
			if (data?.error?.message) message = String(data.error.message);
		} catch {
			// 错误体不是 JSON 时保留默认文案
		}
		const err = new ApiError(res.status, code, message);
		if (!silent) pushToast(message);
		throw err;
	}
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
}

export const coverUrl = (mangaId: number) => `${BASE}/manga/${mangaId}/cover`;

/** NDJSON 流式响应里 partial 之外的两个终态事件形状 */
type StreamTerminal<TFinal> =
	| { type: 'final'; payload: TFinal }
	| { type: 'error'; error: { code: string; message: string } };

export interface ApiStreamOptions<TPartial extends { type: string }> {
	method?: string;
	body?: unknown;
	silent?: boolean;
	signal?: AbortSignal;
	/** 每个 partial 事件到达时调用（按服务端发送顺序） */
	onEvent: (event: TPartial) => void;
}

/**
 * 流式请求：Accept: application/x-ndjson 显式 opt-in，partial 事件逐行回调，
 * resolve 终态 payload（与非流式响应体同形）。服务端不支持流式
 * （content-type 不是 ndjson）时回退整包 JSON，行为与 api() 一致
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
		if (!silent) pushToast(err.message);
		throw err;
	}
	if (!res.ok) {
		let code = 'unknown';
		let message = `请求失败（HTTP ${res.status}）`;
		try {
			const data = await res.json();
			if (data?.error?.code) code = String(data.error.code);
			if (data?.error?.message) message = String(data.error.message);
		} catch {
			// 错误体不是 JSON 时保留默认文案
		}
		const err = new ApiError(res.status, code, message);
		if (!silent) pushToast(message);
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
				throw new ApiError(res.status, 'stream', '流式响应包含无法解析的事件');
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
		if (!silent) pushToast(err.message);
		throw err;
	}
	if (finalPayload === undefined) {
		const err = new ApiError(res.status, 'stream', '流式响应缺少终态事件');
		if (!silent) pushToast(err.message);
		throw err;
	}
	return finalPayload;
}

export const pageUrl = (chapterId: number, index: number) =>
	`${BASE}/chapter/${chapterId}/page/${index}`;

/** 已下载章节的 CBZ 文件地址，配 `<a download>` 直接触发浏览器下载 */
export const chapterFileUrl = (chapterId: number) => `${BASE}/chapter/${chapterId}/file`;

export const sourceIconUrl = (sourceId: number) => `${BASE}/sources/${sourceId}/icon`;
