<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { withToken } from '$lib/api/client';
	import { Button, Modal } from '$lib/components';

	interface Props {
		sourceId: number;
		session: string;
		loginUrl: string;
		/** 完成：收割 cookie 走管线；失败由调用方弹错，视图保持开着 */
		onComplete: () => Promise<void>;
		/** 取消/关闭：清理服务端 context */
		onCancel: () => void;
	}
	let { sourceId, session, loginUrl, onComplete, onCancel }: Props = $props();

	// 与服务端 browser_login 的视口约定一致：坐标按它换算
	const VIEW_W = 1280;
	const VIEW_H = 800;

	const MODIFIER_ALT = 1;
	const MODIFIER_CTRL = 2;
	const MODIFIER_META = 4;
	const MODIFIER_SHIFT = 8;

	function modifiersOf(e: {
		altKey: boolean;
		ctrlKey: boolean;
		metaKey: boolean;
		shiftKey: boolean;
	}): number {
		return (
			(e.altKey ? MODIFIER_ALT : 0) |
			(e.ctrlKey ? MODIFIER_CTRL : 0) |
			(e.metaKey ? MODIFIER_META : 0) |
			(e.shiftKey ? MODIFIER_SHIFT : 0)
		);
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

	let frame = $state('');
	let ended = $state<string | null>(null);
	let busy = $state(false);
	let ws: WebSocket | null = null;
	let imgEl = $state<HTMLImageElement | null>(null);
	let viewEl = $state<HTMLDivElement | null>(null);
	let movePending = 0;

	$effect(() => {
		const sid = sourceId;
		const sess = session;
		const proto = location.protocol === 'https:' ? 'wss' : 'ws';
		const sock = new WebSocket(
			`${proto}://${location.host}${withToken(`/api/v1/sources/${sid}/login/web/browser/${sess}/stream`)}`
		);
		ws = sock;
		sock.onmessage = (e) => {
			if (ws !== sock || typeof e.data !== 'string') return;
			let msg: { type?: string; data?: string; reason?: string };
			try {
				msg = JSON.parse(e.data);
			} catch {
				return;
			}
			if (msg.type === 'frame' && typeof msg.data === 'string') {
				frame = `data:image/jpeg;base64,${msg.data}`;
			} else if (msg.type === 'closed') {
				ended = msg.reason ?? 'closed';
			}
		};
		// 陈旧连接的 close 不得污染新连接的状态
		sock.onclose = () => {
			if (ws === sock) ended = ended ?? 'closed';
		};
		return () => {
			if (ws === sock) ws = null;
			sock.close();
		};
	});

	function send(payload: Record<string, unknown>) {
		if (ws && ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify(payload));
	}

	/** DOM 坐标换算成远端视口坐标 */
	function point(e: { clientX: number; clientY: number }) {
		const rect = imgEl?.getBoundingClientRect();
		if (!rect || rect.width === 0) return null;
		return {
			x: ((e.clientX - rect.left) * VIEW_W) / rect.width,
			y: ((e.clientY - rect.top) * VIEW_H) / rect.height
		};
	}

	function mouseEvent(kind: 'down' | 'up', e: MouseEvent) {
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
</script>

<Modal
	open
	title={$_('browse.browserLoginTitle')}
	onClose={onCancel}
	panelClassName="modal__panel--wide"
>
	<p class="faint">
		{#if ended}
			{$_('browse.sessionEnded')}
		{:else}
			{$_('browse.pageLoadedFrom')}<a href={loginUrl} target="_blank" rel="noreferrer">{loginUrl}</a
			>{$_('browse.browserLoginInstructions')}
		{/if}
	</p>
	<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
	<div
		class="browser-view"
		tabindex="0"
		role="application"
		aria-label={$_('browse.remoteScreen')}
		bind:this={viewEl}
		onkeydown={(e) => {
			e.preventDefault();
			const vk = SPECIAL_KEYS[e.key];
			if (vk !== undefined) {
				send({
					type: 'key',
					event: 'down',
					key: e.key,
					code: e.code,
					windowsVirtualKeyCode: vk,
					modifiers: modifiersOf(e)
				});
			} else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
				send({ type: 'text', text: e.key });
			}
		}}
		onkeyup={(e) => {
			const vk = SPECIAL_KEYS[e.key];
			if (vk !== undefined) {
				e.preventDefault();
				send({
					type: 'key',
					event: 'up',
					key: e.key,
					code: e.code,
					windowsVirtualKeyCode: vk,
					modifiers: modifiersOf(e)
				});
			}
		}}
		onpaste={(e) => {
			e.preventDefault();
			const text = e.clipboardData?.getData('text');
			if (text) send({ type: 'text', text });
		}}
	>
		{#if frame}
			<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
			<img
				bind:this={imgEl}
				class="browser-view__screen"
				src={frame}
				alt={$_('browse.remoteScreen')}
				draggable="false"
				onmousedown={(e) => {
					e.preventDefault();
					// img 不可聚焦，键盘事件挂在 wrapper 上，点击即抢焦
					viewEl?.focus();
					mouseEvent('down', e);
				}}
				onmouseup={(e) => mouseEvent('up', e)}
				onmousemove={(e) => {
					// 悬停事件节流：每帧最多一条，避免 WS 被 mousemove 打满
					if (movePending > 0) return;
					const p = point(e);
					if (!p) return;
					movePending = requestAnimationFrame(() => {
						movePending = 0;
					});
					send({ type: 'mouse', event: 'move', ...p, button: 'none', modifiers: modifiersOf(e) });
				}}
				onwheel={(e) => {
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
				oncontextmenu={(e) => e.preventDefault()}
			/>
		{:else}
			<div class="browser-view__placeholder faint">
				{ended ? $_('browse.sessionEndedShort') : $_('browse.frameLoading')}
			</div>
		{/if}
	</div>

	{#snippet footer()}
		<Button onclick={onCancel}>{$_('common.cancel')}</Button>
		<Button
			variant="primary"
			loading={busy}
			disabled={ended !== null}
			onclick={() => {
				busy = true;
				void onComplete().finally(() => (busy = false));
			}}
		>
			{$_('browse.completeLogin')}
		</Button>
	{/snippet}
</Modal>
