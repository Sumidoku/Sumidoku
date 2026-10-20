import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { Settings } from 'lucide-react';
import {
	invalidateForRefreshes,
	useBasicLogin,
	useClearSourceCache,
	useLoginLogout,
	useNotifySource,
	useOAuthLoginComplete,
	useOAuthLoginStart,
	useResetSourceSettings,
	useSaveSourceSettings,
	useSourceSettings,
	useWebLogin
} from '../../api/hooks';
import type { Setting } from '../../api/types';
import { Button, IconButton, Modal, Spinner, pushToast } from '../../components';
import { SettingItem } from '../settings/SettingItem';
import '../settings.css';

/** Flatten nested settings inside groups/pages, for looking up config items by key */
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
	const { t } = useTranslation();
	const qc = useQueryClient();
	const query = useSourceSettings(sourceId);
	const save = useSaveSourceSettings(sourceId);
	const reset = useResetSourceSettings(sourceId);
	const clearCache = useClearSourceCache(sourceId);
	const login = useBasicLogin(sourceId);
	const webLogin = useWebLogin(sourceId);
	const loginLogout = useLoginLogout(sourceId);
	const oauthStart = useOAuthLoginStart(sourceId);
	const oauthComplete = useOAuthLoginComplete(sourceId);
	const notifySource = useNotifySource(sourceId);
	const [edits, setEdits] = useState<Record<string, unknown>>({});
	const [pending, setPending] = useState<PendingAction>(null);

	if (query.isPending) {
		return (
			<div className="settings-source__loading">
				<Spinner /> {t('common.loading')}
			</div>
		);
	}
	const data = query.data;
	if (!data) return <p className="faint settings-source__empty">{t('sourceSettings.loadFailed')}</p>;
	if (!data.settings || data.settings.length === 0) {
		return <p className="faint settings-source__empty">{t('sourceSettings.noSettings')}</p>;
	}

	const hasEdits = Object.keys(edits).length > 0;

	async function onSave() {
		if (!hasEdits || !data?.settings) return;
		const keys = Object.keys(edits);
		const touched = flattenSettings(data.settings).filter((s) => keys.includes(s.key));
		// Every notification declared by a changed setting is sent to the source (Aidoku: fired as soon as the value changes)
		const notify = touched
			.map((s) => s.notification)
			.filter((n): n is string => typeof n === 'string' && n !== '');
		await save.mutateAsync({ edits, notify });
		setEdits({});
		pushToast(t('sourceSettings.saved'), 'info');
		invalidateForRefreshes(qc, sourceId, touched.flatMap((s) => s.refreshes ?? []));
	}

	/** Pressing a Button setting sends its notification to the source, then refreshes the declared data */
	async function onPressButton(setting: Setting) {
		if (!setting.notification) return;
		await notifySource.mutateAsync(setting.notification);
		invalidateForRefreshes(qc, sourceId, setting.refreshes ?? []);
	}

	async function onReset() {
		await reset.mutateAsync();
		setEdits({});
		setPending(null);
		pushToast(t('sourceSettings.resetDone'), 'info');
	}

	async function onClearCache() {
		await clearCache.mutateAsync();
		setPending(null);
		pushToast(t('sourceSettings.cacheCleared'), 'info');
	}

	async function onSubmitLogin(key: string, username: string, password: string) {
		await login.mutateAsync({ key, username, password });
		setEdits({});
		pushToast(t('sourceSettings.loginSuccess'), 'info');
	}

	async function onSubmitWebLogin(key: string, cookies: Record<string, string>, localStorage: Record<string, string>) {
		await webLogin.mutateAsync({ key, cookies, localStorage });
		setEdits({});
		pushToast(t('sourceSettings.loginSuccess'), 'info');
	}

	async function onLogoutLogin(key: string, clearCookies: boolean) {
		await loginLogout.mutateAsync({ key, clearCookies });
		setEdits({});
		pushToast(t('sourceSettings.loggedOut'), 'info');
	}

	async function onCompleteOAuth(key: string, url: string) {
		await oauthComplete.mutateAsync({ key, url });
		setEdits({});
		pushToast(t('sourceSettings.loginSuccess'), 'info');
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
					onPressButton={onPressButton}
					onSubmitLogin={onSubmitLogin}
					onSubmitWebLogin={onSubmitWebLogin}
					onLogoutLogin={onLogoutLogin}
					onStartOAuth={(key) => oauthStart.mutateAsync(key)}
					onCompleteOAuth={onCompleteOAuth}
				/>
			))}
			<div className="settings-source__save">
				<Button variant="ghost" size="sm" onClick={() => setPending('reset')}>
					{t('sourceSettings.resetSettings')}
				</Button>
				<Button variant="ghost" size="sm" onClick={() => setPending('clear')}>
					{t('sourceSettings.clearLoginCache')}
				</Button>
				<Button
					variant="primary"
					size="sm"
					loading={save.isPending}
					disabled={!hasEdits}
					onClick={() => void onSave()}
				>
					{t('common.save')}
				</Button>
			</div>
			<Modal
				open={pending === 'reset'}
				title={t('sourceSettings.resetSettings')}
				onClose={() => setPending(null)}
				footer={
					<>
						<Button onClick={() => setPending(null)}>{t('common.cancel')}</Button>
						<Button variant="primary" loading={reset.isPending} onClick={() => void onReset()}>
							{t('sourceSettings.reset')}
						</Button>
					</>
				}
			>
				<p>{t('sourceSettings.resetConfirm')}</p>
			</Modal>
			<Modal
				open={pending === 'clear'}
				title={t('sourceSettings.clearLoginCache')}
				onClose={() => setPending(null)}
				footer={
					<>
						<Button onClick={() => setPending(null)}>{t('common.cancel')}</Button>
						<Button variant="primary" loading={clearCache.isPending} onClick={() => void onClearCache()}>
							{t('sourceSettings.clear')}
						</Button>
					</>
				}
			>
				<p>{t('sourceSettings.clearConfirm')}</p>
			</Modal>
		</div>
	);
}

/** The gear at the right of an installed-source card: settings live in a modal, config items come from the source package */
export function SourceSettingsButton({ sourceId, sourceName }: { sourceId: number; sourceName: string }) {
	const { t } = useTranslation();
	const [open, setOpen] = useState(false);

	return (
		<>
			<IconButton
				label={t('sourceSettings.title', { name: sourceName })}
				onClick={(e) => {
					e.stopPropagation();
					setOpen(true);
				}}
			>
				<Settings size={17} />
			</IconButton>
			<Modal
				open={open}
				title={t('sourceSettings.title', { name: sourceName })}
				onClose={() => setOpen(false)}
				panelClassName="modal__panel--wide"
			>
				{open && <SourceSettingsPanel sourceId={sourceId} />}
			</Modal>
		</>
	);
}
