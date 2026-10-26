import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useRegisterInternalSource } from '../../api/hooks';
import { ApiError } from '../../api/client';
import { translateApiError } from '../../i18n';
import { Button, Input, LocalDirsEditor, Modal, SegmentedControl, Switch, pushToast } from '../../components';
import '../settings.css';

type InternalKind = 'opds' | 'local';

/**
 * Registration of built-in (non-wasm) source instances. The kind picker lists what the server's
 * /sources/internal endpoint dispatches; a future kind is one more option plus its form here.
 *
 * The local branch mirrors the local source settings panel: roots, the oneshots marker, exclusion
 * substrings and the three scan/analysis toggles. Fields the user leaves at the documented server
 * config defaults are omitted so the config layer keeps governing; only explicit deviations are sent.
 */
export function InternalSourceModal({ open, onClose }: { open: boolean; onClose: () => void }) {
	const { t } = useTranslation();
	const register = useRegisterInternalSource();
	const [kind, setKind] = useState<InternalKind>('opds');
	const [name, setName] = useState('');
	// opds fields
	const [url, setUrl] = useState('');
	const [username, setUsername] = useState('');
	const [password, setPassword] = useState('');
	// local fields
	const [dirs, setDirs] = useState<string[]>(['']);
	const [oneshots, setOneshots] = useState('');
	const [exclusions, setExclusions] = useState<string[]>(['']);
	// defaults mirror the server config layer (local_scan_force_modified_time / local_analyze_pages /
	// local_analyze_metadata); only toggled-away values are sent, leaving unset fields to config fallback
	const [forceModifiedTime, setForceModifiedTime] = useState(false);
	const [analyzePages, setAnalyzePages] = useState(true);
	const [analyzeMetadata, setAnalyzeMetadata] = useState(true);

	function reset() {
		setKind('opds');
		setName('');
		setUrl('');
		setUsername('');
		setPassword('');
		setDirs(['']);
		setOneshots('');
		setExclusions(['']);
		setForceModifiedTime(false);
		setAnalyzePages(true);
		setAnalyzeMetadata(true);
	}

	const localPaths = dirs.map((d) => d.trim()).filter(Boolean);
	const canSubmit = name.trim() !== '' && (kind === 'opds' ? url.trim() !== '' : localPaths.length > 0);

	async function submit() {
		try {
			const source = await register.mutateAsync(
				kind === 'opds'
					? {
							type: 'opds',
							name: name.trim(),
							url: url.trim(),
							...(username.trim() ? { username: username.trim(), password } : {})
						}
					: {
							type: 'local',
							name: name.trim(),
							paths: localPaths,
							...(oneshots.trim() ? { oneshotsDir: oneshots.trim() } : {}),
							...(() => {
								const cleaned = exclusions.map((x) => x.trim()).filter(Boolean);
								return cleaned.length ? { exclusions: cleaned } : {};
							})(),
							...(forceModifiedTime ? { forceModifiedTime: true } : {}),
							...(analyzePages ? {} : { analyzePages: false }),
							...(analyzeMetadata ? {} : { analyzeMetadata: false })
						}
			);
			pushToast(t('browse.internalAdded', { name: source.name }), 'info');
			reset();
			onClose();
		} catch (e) {
			if (e instanceof ApiError && e.status === 400) pushToast(translateApiError(e.code, e.message));
			else pushToast(t('browse.addFailed'));
		}
	}

	return (
		<Modal
			open={open}
			title={t('browse.addInternal')}
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>{t('common.cancel')}</Button>
					<Button
						variant="primary"
						loading={register.isPending}
						disabled={!canSubmit}
						onClick={() => void submit()}
					>
						{t('browse.add')}
					</Button>
				</>
			}
		>
			<div className="opds-form">
				<SegmentedControl
					options={[
						{ value: 'opds', label: t('browse.internalTypeOpds') },
						{ value: 'local', label: t('browse.internalTypeLocal') }
					]}
					value={kind}
					onChange={setKind}
				/>
				<Input
					placeholder={t('browse.opdsNamePlaceholder')}
					value={name}
					onChange={(e) => setName(e.currentTarget.value)}
				/>
				{kind === 'opds' ? (
					<>
						<Input
							placeholder={t('browse.opdsUrlPlaceholder')}
							value={url}
							onChange={(e) => setUrl(e.currentTarget.value)}
						/>
						<Input
							placeholder={t('browse.opdsUsernamePlaceholder')}
							autoComplete="off"
							value={username}
							onChange={(e) => setUsername(e.currentTarget.value)}
						/>
						<Input
							type="password"
							placeholder={t('browse.opdsPasswordPlaceholder')}
							autoComplete="new-password"
							value={password}
							onChange={(e) => setPassword(e.currentTarget.value)}
						/>
						<p className="muted">{t('browse.opdsHint')}</p>
					</>
				) : (
					<>
						<LocalDirsEditor dirs={dirs} onChange={setDirs} />
						<Input
							placeholder={t('browse.localOneshotsPlaceholder')}
							value={oneshots}
							onChange={(e) => setOneshots(e.currentTarget.value)}
						/>
						<p className="settings-local__hint">{t('sourceSettings.localOneshotsDirDesc')}</p>
						<LocalDirsEditor
							dirs={exclusions}
							onChange={setExclusions}
							placeholder={t('sourceSettings.localExclusionPlaceholder')}
							addLabel={t('sourceSettings.addExclusion')}
						/>
						<p className="settings-local__hint">{t('sourceSettings.localExclusionsDesc')}</p>
						<div className="settings-row">
							<span className="settings-row__label">{t('sourceSettings.localForceModifiedTime')}</span>
							<Switch
								checked={forceModifiedTime}
								onChange={setForceModifiedTime}
								label={t('sourceSettings.localForceModifiedTime')}
							/>
						</div>
						<div className="settings-row">
							<span className="settings-row__label">{t('sourceSettings.localAnalyzePages')}</span>
							<Switch
								checked={analyzePages}
								onChange={setAnalyzePages}
								label={t('sourceSettings.localAnalyzePages')}
							/>
						</div>
						<div className="settings-row">
							<span className="settings-row__label">{t('sourceSettings.localAnalyzeMetadata')}</span>
							<Switch
								checked={analyzeMetadata}
								onChange={setAnalyzeMetadata}
								label={t('sourceSettings.localAnalyzeMetadata')}
							/>
						</div>
						<p className="settings-local__hint">{t('sourceSettings.localScanFlagsDesc')}</p>
					</>
				)}
			</div>
		</Modal>
	);
}
