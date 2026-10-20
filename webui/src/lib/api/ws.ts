// WebSocket 订阅封装：自动重连、JSON 解析、退订清理。
import { withToken } from './client';

export interface WSHandlers {
	/** 每帧 JSON 已解析后的回调，帧结构由调用方自行收窄 */
	onMessage: (data: unknown) => void;
	/** 连接状态变化：true 在线 / false 离线 */
	onStatus?: (online: boolean) => void;
}

/** 订阅一个 WS 端点，返回退订函数（组件卸载时调用）。断线后 3s 自动重连，直到退订 */
export function subscribeWS(path: string, handlers: WSHandlers): () => void {
	let ws: WebSocket | null = null;
	let closed = false;
	let timer: ReturnType<typeof setTimeout> | undefined;

	function connect() {
		const proto = location.protocol === 'https:' ? 'wss' : 'ws';
		ws = new WebSocket(`${proto}://${location.host}${withToken(path)}`);
		ws.onopen = () => handlers.onStatus?.(true);
		ws.onmessage = (e) => {
			if (typeof e.data !== 'string') return;
			try {
				handlers.onMessage(JSON.parse(e.data));
			} catch {
				// 非 JSON 帧不是协议的一部分，直接忽略
			}
		};
		ws.onclose = () => {
			handlers.onStatus?.(false);
			if (!closed) timer = setTimeout(connect, 3000);
		};
		// error 之后必跟 close，重连统一走 onclose 一条路
		ws.onerror = () => ws?.close();
	}

	connect();

	return () => {
		closed = true;
		clearTimeout(timer);
		ws?.close();
	};
}
