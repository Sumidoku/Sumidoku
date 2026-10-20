import { get } from 'svelte/store';
import { _ } from 'svelte-i18n';
import { pushToast } from '$lib/components/toast';

const BASE = '/api/v1';

/** 非组件环境里的翻译取值（i18n 在根布局已 await 初始化，到这里字典必就绪） */
function t(key: string, values?: Record<string, string | number | boolean | null | undefined>): string {
	try {
		return get(_)(key, values ? { values } : undefined) as string;
	} catch {
		return key;
	}
}
const TOKEN_KEY = 'sumidoku.auth.token';
let memoryToken: string | null = null;

export function getToken(): string | null {
	try {
		return localStorage.getItem(TOKEN_KEY) ?? memoryToken;
	} catch {
		return memoryToken;
	}
}

export function setToken(token: string) {
	memoryToken = token;
	try {
		localStorage.setItem(TOKEN_KEY, token);
	} catch {
		// 隐私模式写不进去，靠内存态撑本次会话（刷新后需重新登录）
	}
}

export function clearToken() {
	memoryToken = null;
	try {
		localStorage.removeItem(TOKEN_KEY);
	} catch {
		// 同上
	}
}

/** 401 归一处理：清 token 跳登录页；登录/改密码请求由页面本地处理（防循环、防误踢） */
function handleUnauthorized(path: string) {
	if (location.pathname === '/login' || path === '/auth/login' || path === '/auth/password') return;
	clearToken();
	location.assign('/login');
}

function authHeaders(): Record<string, string> {
	const token = getToken();
	return token ? { Authorization: `Bearer ${token}` } : {};
}

/** <img>/<a>/WebSocket 发不了 Authorization 头，媒体与流地址带 query token */
export function withToken(url: string): string {
	const token = getToken();
	if (!token) return url;
	const sep = url.includes('?') ? '&' : '?';
	return `${url}${sep}access_token=${encodeURIComponent(token)}`;
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
	/** 探测性请求（如 404 表示不支持 listings）不弹错误提示 */
	silent?: boolean;
}

export async function api<T>(path: string, opts: ApiOptions = {}): Promise<T> {
	const { method = 'GET', body, silent = false } = opts;
	// 二进制上传（.aib）不走 JSON 编码
	const raw = body instanceof Blob || body instanceof ArrayBuffer;
	let res: Response;
	try {
		res = await fetch(BASE + path, {
			method,
			headers:
				body !== undefined
					? { 'Content-Type': raw ? 'application/octet-stream' : 'application/json', ...authHeaders() }
					: authHeaders(),
			body: body !== undefined ? (raw ? (body as BodyInit) : JSON.stringify(body)) : undefined
		});
	} catch {
		const err = new ApiError(0, 'network', t('error.network'));
		if (!silent) pushToast(err.message);
		throw err;
	}
	if (!res.ok) {
		let code = 'unknown';
		let message = t('error.http', { status: res.status });
		try {
			const data = await res.json();
			if (data?.error?.code) code = String(data.error.code);
			if (data?.error?.message) message = String(data.error.message);
		} catch {
			// 错误体不是 JSON 时保留默认文案
		}
		const err = new ApiError(res.status, code, message);
		if (res.status === 401) handleUnauthorized(path);
		if (!silent) pushToast(message);
		throw err;
	}
	if (res.status === 204) return undefined as T;
	return (await res.json()) as T;
}

/** v 是版本标记（传 manga.cover）：换封面后端点 URL 不变而响应 max-age=86400 可缓存，没它浏览器一直用旧图 */
export const coverUrl = (mangaId: number, v?: string | null) => {
	const base = withToken(`${BASE}/manga/${mangaId}/cover`);
	// base 不一定带 query（没 token 时 withToken 原样返回），分隔符要自己判
	const sep = base.includes('?') ? '&' : '?';
	return v ? `${base}${sep}v=${encodeURIComponent(v)}` : base;
};

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
				...authHeaders(),
				...(body !== undefined ? { 'Content-Type': 'application/json' } : {})
			},
			body: body !== undefined ? JSON.stringify(body) : undefined,
			signal
		});
	} catch (e) {
		if (e instanceof DOMException && e.name === 'AbortError') throw e;
		const err = new ApiError(0, 'network', t('error.network'));
		if (!silent) pushToast(err.message);
		throw err;
	}
	if (!res.ok) {
		let code = 'unknown';
		let message = t('error.http', { status: res.status });
		try {
			const data = await res.json();
			if (data?.error?.code) code = String(data.error.code);
			if (data?.error?.message) message = String(data.error.message);
		} catch {
			// 错误体不是 JSON 时保留默认文案
		}
		const err = new ApiError(res.status, code, message);
		if (res.status === 401) handleUnauthorized(path);
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
				throw new ApiError(res.status, 'stream', t('error.streamUnparseable'));
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
		const err = new ApiError(res.status, 'stream', t('error.streamMissingFinal'));
		if (!silent) pushToast(err.message);
		throw err;
	}
	return finalPayload;
}

export const pageUrl = (chapterId: number, index: number, width?: number) =>
	withToken(`${BASE}/chapter/${chapterId}/page/${index}${width ? `?width=${width}` : ''}`);

/** 已下载章节的 CBZ 文件地址，配 `<a download>` 直接触发浏览器下载 */
export const chapterFileUrl = (chapterId: number) =>
	withToken(`${BASE}/chapter/${chapterId}/file`);

/** 全量备份下载地址，配 `<a download>` 触发浏览器保存 */
export const backupUrl = () => withToken(`${BASE}/backup`);

export const sourceIconUrl = (sourceId: number) =>
	withToken(`${BASE}/sources/${sourceId}/icon`);

/** 源图床防盗链要的头只有服务端能按源声明带上，browse/home 封面经服务端代理取回 */
export const sourceImageUrl = (sourceId: number, url: string) =>
	withToken(`${BASE}/sources/${sourceId}/image?url=${encodeURIComponent(url)}`);
