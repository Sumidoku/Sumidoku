import { useState } from 'react';
import { Settings } from 'lucide-react';
import {
	useBasicLogin,
	useClearSourceCache,
	useLoginLogout,
	useResetSourceSettings,
	useSaveSourceSettings,
	useSourceSettings,
	useWebLogin
} from '../../api/hooks';
import type { Setting } from '../../api/types';
import { Button, IconButton, Modal, Spinner, pushToast } from '../../components';
import { SettingItem } from '../settings/SettingItem';
import '../settings.css';

/** 摊平 group/page 里的嵌套设置，按 key 找配置项用 */
function flattenSettings(list: Setting[]): Setting[] {
	return list.flatMap((s) => {
		const v = s.value;
		if (typeof v === 'object' && 'Group' in v) return flattenSettings(v.Group.items);
		if (typeof v === 'object' && 'Page' in v) return flattenSettings(v.Page.items);
		return [s];
	});
}

type PendingAction = 'reset' | 'clear' | null;

function SourceSettingsPanel({ sourceId }: { sourceId: number }) {
	const query = useSourceSettings(sourceId);
	const save = useSaveSourceSettings(sourceId);
	const reset = useResetSourceSettings(sourceId);
	const clearCache = useClearSourceCache(sourceId);
	const login = useBasicLogin(sourceId);
	const webLogin = useWebLogin(sourceId);
	const loginLogout = useLoginLogout(sourceId);
	const [edits, setEdits] = useState<Record<string, unknown>>({});
	const [pending, setPending] = useState<PendingAction>(null);

	if (query.isPending) {
		return (
			<div className="settings-source__loading">
				<Spinner /> 加载中…
			</div>
		);
	}
	const data = query.data;
	if (!data) return <p className="faint settings-source__empty">设置加载失败</p>;
	if (!data.settings || data.settings.length === 0) {
		return <p className="faint settings-source__empty">该源没有可配置项</p>;
	}

	const hasEdits = Object.keys(edits).length > 0;

	async function onSave() {
		if (!hasEdits || !data?.settings) return;
		const keys = Object.keys(edits);
		await save.mutateAsync(edits);
		setEdits({});
		pushToast('已保存', 'info');
		const touched = flattenSettings(data.settings).filter((s) => keys.includes(s.key));
		if (touched.some((s) => s.refreshes && s.refreshes.length > 0)) {
			pushToast('该源会在后台刷新设置', 'info');
		}
	}

	async function onReset() {
		await reset.mutateAsync();
		setEdits({});
		setPending(null);
		pushToast('已重置为默认设置', 'info');
	}

	async function onClearCache() {
		await clearCache.mutateAsync();
		setPending(null);
		pushToast('已清除登录缓存', 'info');
	}

	async function onSubmitLogin(key: string, username: string, password: string) {
		await login.mutateAsync({ key, username, password });
		setEdits({});
		pushToast('登录成功', 'info');
	}

	async function onSubmitWebLogin(key: string, cookies: Record<string, string>, localStorage: Record<string, string>) {
		await webLogin.mutateAsync({ key, cookies, localStorage });
		setEdits({});
		pushToast('登录成功', 'info');
	}

	async function onLogoutLogin(key: string, clearCookies: boolean) {
		await loginLogout.mutateAsync({ key, clearCookies });
		setEdits({});
		pushToast('已登出', 'info');
	}

	return (
		<div className="settings-source__body">
			{data.settings.map((setting) => (
				<SettingItem
					key={setting.key}
					setting={setting}
					values={data.values}
					edits={edits}
					onEdit={(k, v) => setEdits((cur) => ({ ...cur, [k]: v }))}
					onSubmitLogin={onSubmitLogin}
					onSubmitWebLogin={onSubmitWebLogin}
					onLogoutLogin={onLogoutLogin}
				/>
			))}
			<div className="settings-source__save">
				<Button variant="ghost" size="sm" onClick={() => setPending('reset')}>
					重置设置
				</Button>
				<Button variant="ghost" size="sm" onClick={() => setPending('clear')}>
					清除登录缓存
				</Button>
				<Button
					variant="primary"
					size="sm"
					loading={save.isPending}
					disabled={!hasEdits}
					onClick={() => void onSave()}
				>
					保存
				</Button>
			</div>
			<Modal
				open={pending === 'reset'}
				title="重置设置"
				onClose={() => setPending(null)}
				footer={
					<>
						<Button onClick={() => setPending(null)}>取消</Button>
						<Button variant="primary" loading={reset.isPending} onClick={() => void onReset()}>
							重置
						</Button>
					</>
				}
			>
				<p>重置该图源的全部设置为默认值？未保存的修改会丢弃，登录信息也会一并清除。</p>
			</Modal>
			<Modal
				open={pending === 'clear'}
				title="清除登录缓存"
				onClose={() => setPending(null)}
				footer={
					<>
						<Button onClick={() => setPending(null)}>取消</Button>
						<Button variant="primary" loading={clearCache.isPending} onClick={() => void onClearCache()}>
							清除
						</Button>
					</>
				}
			>
				<p>清除该图源的登录 cookie 并重启实例？需要重新登录。</p>
			</Modal>
		</div>
	);
}

/** 已安装源卡片右侧的齿轮：设置放进弹窗，配置项由源包提供 */
export function SourceSettingsButton({ sourceId, sourceName }: { sourceId: number; sourceName: string }) {
	const [open, setOpen] = useState(false);

	return (
		<>
			<IconButton
				label={`${sourceName} 设置`}
				onClick={(e) => {
					e.stopPropagation();
					setOpen(true);
				}}
			>
				<Settings size={17} />
			</IconButton>
			<Modal
				open={open}
				title={`${sourceName} 设置`}
				onClose={() => setOpen(false)}
				panelClassName="modal__panel--wide"
			>
				{open && <SourceSettingsPanel sourceId={sourceId} />}
			</Modal>
		</>
	);
}
