// WebSocket subscription wrapper: auto-reconnect, JSON parsing, unsubscribe cleanup.
import { withToken } from './client';

export interface WSHandlers {
	/** Called with the parsed JSON of each frame; the frame shape is narrowed by the caller */
	onMessage: (data: unknown) => void;
	/** Connection status change: true online / false offline */
	onStatus?: (online: boolean) => void;
}

/** Subscribe to a WS endpoint, returning an unsubscribe function (call on component unmount). Reconnects automatically 3s after disconnect until unsubscribed */
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
				// Non-JSON frames are not part of the protocol; ignore them
			}
		};
		ws.onclose = () => {
			handlers.onStatus?.(false);
			if (!closed) timer = setTimeout(connect, 3000);
		};
		// An error is always followed by close, so reconnects all go through onclose
		ws.onerror = () => ws?.close();
	}

	connect();

	return () => {
		closed = true;
		clearTimeout(timer);
		ws?.close();
	};
}
