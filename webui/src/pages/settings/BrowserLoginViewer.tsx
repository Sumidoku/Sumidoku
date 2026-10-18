import { useEffect, useRef, useState } from 'react';
import { withToken } from '../../api/client';
import { Button, Modal } from '../../components';

/** 与服务端 browser_login 的视口约定一致：坐标按它换算 */
const VIEW_W = 1280;
const VIEW_H = 800;

const MODIFIER_ALT = 1;
const MODIFIER_CTRL = 2;
const MODIFIER_META = 4;
const MODIFIER_SHIFT = 8;

function modifiersOf(e: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }): number {
	return (e.altKey ? MODIFIER_ALT : 0) | (e.ctrlKey ? MODIFIER_CTRL : 0) | (e.metaKey ? MODIFIER_META : 0) | (e.shiftKey ? MODIFIER_SHIFT : 0);
}

const BUTTONS = ['left', 'middle', 'right'] as const;

/** 特殊键走 keyDown/keyUp（windowsVirtualKeyCode），可打印字符走 insertText */
const SPECIAL_KEYS: Record<string, number> = {
	Enter: 13,
	Backspace: 8,
	Tab: 9,
	Escape: 27,
	Delete: 46,
	ArrowLeft: 37,
	ArrowUp: 38,
	ArrowRight: 39,
	ArrowDown: 40,
	Home: 36,
	End: 35,
	PageUp: 33,
	PageDown: 34
};

export interface BrowserLoginViewerProps {
	sourceId: number;
	session: string;
	loginUrl: string;
	/** 完成：收割 cookie 走管线；失败由调用方弹错，视图保持开着 */
	onComplete: () => Promise<void>;
	/** 取消/关闭：清理服务端 context */
	onCancel: () => void;
}

/**
 * sidecar 交互登录的实时视图：WS 收 screencast 帧渲染成 img，
 * 鼠标/键盘事件按远端视口换算后经同一条 WS 回灌。不做断线重连——
 * 会话是一次性的，重连上的也是终结态
 */
export function BrowserLoginViewer({ sourceId, session, loginUrl, onComplete, onCancel }: BrowserLoginViewerProps) {
	const [frame, setFrame] = useState('');
	const [ended, setEnded] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const wsRef = useRef<WebSocket | null>(null);
	const imgRef = useRef<HTMLImageElement | null>(null);
	const viewRef = useRef<HTMLDivElement | null>(null);
	const movePendingRef = useRef(0);

	useEffect(() => {
		const proto = location.protocol === 'https:' ? 'wss' : 'ws';
		const ws = new WebSocket(
			`${proto}://${location.host}${withToken(`/api/v1/sources/${sourceId}/login/web/browser/${session}/stream`)}`
		);
		wsRef.current = ws;
		ws.onmessage = (e) => {
			if (wsRef.current !== ws || typeof e.data !== 'string') return;
			let msg: { type?: string; data?: string; reason?: string };
			try {
				msg = JSON.parse(e.data);
			} catch {
				return;
			}
			if (msg.type === 'frame' && typeof msg.data === 'string') {
				setFrame(`data:image/jpeg;base64,${msg.data}`);
			} else if (msg.type === 'closed') {
				setEnded(msg.reason ?? 'closed');
			}
		};
		// StrictMode 双挂载会关掉第一条连接：陈旧连接的 close 不得污染新连接的状态
		ws.onclose = () => {
			if (wsRef.current === ws) setEnded((cur) => cur ?? 'closed');
		};
		return () => {
			if (wsRef.current === ws) wsRef.current = null;
			ws.close();
		};
	}, [sourceId, session]);

	function send(payload: Record<string, unknown>) {
		const ws = wsRef.current;
		if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(payload));
	}

	/** DOM 坐标换算成远端视口坐标 */
	function point(e: { clientX: number; clientY: number }) {
		const rect = imgRef.current?.getBoundingClientRect();
		if (!rect || rect.width === 0) return null;
		return {
			x: ((e.clientX - rect.left) * VIEW_W) / rect.width,
			y: ((e.clientY - rect.top) * VIEW_H) / rect.height
		};
	}

	function mouseEvent(kind: 'down' | 'up', e: React.MouseEvent) {
		const p = point(e);
		if (!p) return;
		send({
			type: 'mouse',
			event: kind,
			...p,
			button: BUTTONS[e.button] ?? 'left',
			clickCount: 1,
			modifiers: modifiersOf(e)
		});
	}

	return (
		<Modal
			open
			title="在浏览器中完成登录"
			onClose={onCancel}
			panelClassName="modal__panel--wide"
			footer={
				<>
					<Button onClick={onCancel}>取消</Button>
					<Button
						variant="primary"
						loading={busy}
						disabled={ended !== null}
						onClick={() => {
							setBusy(true);
							void onComplete().finally(() => setBusy(false));
						}}
					>
						完成登录
					</Button>
				</>
			}
		>
			<p className="faint">
				{ended
					? '会话已结束，请关闭后重新打开'
					: '页面加载自 '}
				{!ended && (
					<a href={loginUrl} target="_blank" rel="noreferrer">
						{loginUrl}
					</a>
				)}
				{!ended && '，在下方画面里完成登录（含验证码/两步验证），然后点「完成登录」'}
			</p>
			<div
				className="browser-view"
				tabIndex={0}
				role="application"
				aria-label="远程浏览器画面"
				ref={viewRef}
				onKeyDown={(e) => {
					e.preventDefault();
					const vk = SPECIAL_KEYS[e.key];
					if (vk !== undefined) {
						send({ type: 'key', event: 'down', key: e.key, code: e.code, windowsVirtualKeyCode: vk, modifiers: modifiersOf(e) });
					} else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
						send({ type: 'text', text: e.key });
					}
				}}
				onKeyUp={(e) => {
					const vk = SPECIAL_KEYS[e.key];
					if (vk !== undefined) {
						e.preventDefault();
						send({ type: 'key', event: 'up', key: e.key, code: e.code, windowsVirtualKeyCode: vk, modifiers: modifiersOf(e) });
					}
				}}
				onPaste={(e) => {
					e.preventDefault();
					const text = e.clipboardData.getData('text');
					if (text) send({ type: 'text', text });
				}}
			>
				{frame ? (
					<img
						ref={imgRef}
						className="browser-view__screen"
						src={frame}
						alt="远程浏览器画面"
						draggable={false}
						onMouseDown={(e) => {
							e.preventDefault();
							// img 不可聚焦，键盘事件挂在 wrapper 上，点击即抢焦
							viewRef.current?.focus();
							mouseEvent('down', e);
						}}
						onMouseUp={(e) => mouseEvent('up', e)}
						onMouseMove={(e) => {
							// 悬停事件节流：每帧最多一条，避免 WS 被 mousemove 打满
							if (movePendingRef.current > 0) return;
							const p = point(e);
							if (!p) return;
							movePendingRef.current = requestAnimationFrame(() => {
								movePendingRef.current = 0;
							});
							send({ type: 'mouse', event: 'move', ...p, button: 'none', modifiers: modifiersOf(e) });
						}}
						onWheel={(e) => {
							const p = point(e);
							if (!p) return;
							// CDP 的 deltaY 与 DOM 同号（正值 = 向下滚）
							send({
								type: 'mouse',
								event: 'wheel',
								...p,
								button: 'none',
								deltaX: e.deltaX,
								deltaY: e.deltaY,
								modifiers: modifiersOf(e)
							});
						}}
						onContextMenu={(e) => e.preventDefault()}
					/>
				) : (
					<div className="browser-view__placeholder faint">{ended ? '会话已结束' : '画面加载中…'}</div>
				)}
			</div>
		</Modal>
	);
}
