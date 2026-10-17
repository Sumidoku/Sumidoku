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

export const pageUrl = (chapterId: number, index: number) =>
	`${BASE}/chapter/${chapterId}/page/${index}`;

/** 已下载章节的 CBZ 文件地址，配 `<a download>` 直接触发浏览器下载 */
export const chapterFileUrl = (chapterId: number) => `${BASE}/chapter/${chapterId}/file`;

export const sourceIconUrl = (sourceId: number) => `${BASE}/sources/${sourceId}/icon`;
