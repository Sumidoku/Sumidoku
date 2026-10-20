import { useState } from 'react';
import { Navigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { KeyRound, Trash2, UserPlus } from 'lucide-react';
import { ApiError } from '../api/client';
import { useCreateUser, useDeleteUser, useIsAdmin, useUpdateUser, useUsers, useWhoami } from '../api/hooks';
import type { ManagedUser } from '../api/types';
import { translateApiError } from '../i18n';
import { Badge, Button, IconButton, Input, Modal, PageHeader, Select, Skeleton, pushToast } from '../components';
import './users.css';

function roleLabel(t: TFunction, role: string): string {
	if (role === 'admin') return t('users.roleAdmin');
	if (role === 'reader') return t('users.roleReader');
	return role;
}

function toastError(t: TFunction, e: unknown) {
	if (e instanceof ApiError && e.status === 409) pushToast(t('users.usernameTaken'));
	else if (e instanceof ApiError && e.status === 400) pushToast(translateApiError(e.code, e.message));
	else pushToast(t('users.operationFailed'));
}

function CreateUserModal({ open, onClose }: { open: boolean; onClose: () => void }) {
	const { t } = useTranslation();
	const create = useCreateUser();
	const [username, setUsername] = useState('');
	const [password, setPassword] = useState('');
	const [role, setRole] = useState('reader');

	function reset() {
		setUsername('');
		setPassword('');
		setRole('reader');
	}

	async function submit() {
		try {
			await create.mutateAsync({ username: username.trim(), password, role });
			pushToast(t('users.created', { username: username.trim() }), 'info');
			reset();
			onClose();
		} catch (e) {
			toastError(t, e);
		}
	}

	return (
		<Modal
			open={open}
			title={t('users.createTitle')}
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>{t('common.cancel')}</Button>
					<Button
						variant="primary"
						loading={create.isPending}
						disabled={!username.trim() || !password}
						onClick={() => void submit()}
					>
						{t('users.create')}
					</Button>
				</>
			}
		>
			<div className="users-form">
				<Input
					placeholder={t('users.username')}
					value={username}
					onChange={(e) => setUsername(e.currentTarget.value)}
				/>
				<Input
					type="password"
					placeholder={t('users.initialPassword')}
					autoComplete="new-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				<Select value={role} onChange={(e) => setRole(e.currentTarget.value)}>
					<option value="reader">{t('users.roleReaderDesc')}</option>
					<option value="admin">{t('users.roleAdminDesc')}</option>
				</Select>
			</div>
		</Modal>
	);
}

function ResetPasswordModal({ user, onClose }: { user: ManagedUser | null; onClose: () => void }) {
	const { t } = useTranslation();
	const update = useUpdateUser();
	const [password, setPassword] = useState('');

	async function submit() {
		if (!user) return;
		try {
			await update.mutateAsync({ id: user.id, password });
			pushToast(t('users.passwordReset', { username: user.username }), 'info');
			setPassword('');
			onClose();
		} catch (e) {
			toastError(t, e);
		}
	}

	return (
		<Modal
			open={user !== null}
			title={t('users.resetPasswordTitle', { username: user?.username ?? '' })}
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>{t('common.cancel')}</Button>
					<Button
						variant="primary"
						loading={update.isPending}
						disabled={!password}
						onClick={() => void submit()}
					>
						{t('users.reset')}
					</Button>
				</>
			}
		>
			<div className="users-form">
				<Input
					type="password"
					placeholder={t('users.newPassword')}
					autoComplete="new-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				<p className="faint">{t('users.resetPasswordHint')}</p>
			</div>
		</Modal>
	);
}

function UserRow({ user, selfId, onResetPassword }: { user: ManagedUser; selfId?: number; onResetPassword: () => void }) {
	const { t } = useTranslation();
	const update = useUpdateUser();
	const del = useDeleteUser();
	const [confirming, setConfirming] = useState(false);
	// user 1 is the built-in admin: the server refuses to delete it (400), so the entry is simply not offered
	const builtin = user.id === 1;

	async function changeRole(role: string) {
		try {
			await update.mutateAsync({ id: user.id, role });
			pushToast(t('users.roleChanged', { username: user.username, role: roleLabel(t, role) }), 'info');
		} catch (e) {
			toastError(t, e);
		}
	}

	return (
		<div className="users-row">
			<div className="users-row__main">
				<span className="users-row__name">
					{user.username}
					{user.id === selfId && <Badge>{t('users.currentAccount')}</Badge>}
				</span>
				<span className="faint">
					{t('users.rowMeta', { role: roleLabel(t, user.role), date: new Date(user.createdAt * 1000).toLocaleString() })}
				</span>
			</div>
			<div className="users-row__ops">
				<Select
					value={user.role}
					disabled={update.isPending}
					onChange={(e) => void changeRole(e.currentTarget.value)}
				>
					<option value="admin">{t('users.roleAdmin')}</option>
					<option value="reader">{t('users.roleReader')}</option>
				</Select>
				<IconButton label={t('users.resetPasswordLabel', { username: user.username })} onClick={onResetPassword}>
					<KeyRound size={16} />
				</IconButton>
				{!builtin && (
					<IconButton label={t('users.deleteUserLabel', { username: user.username })} variant="danger" onClick={() => setConfirming(true)}>
						<Trash2 size={16} />
					</IconButton>
				)}
			</div>

			<Modal
				open={confirming}
				title={t('users.deleteUserTitle', { username: user.username })}
				onClose={() => setConfirming(false)}
				footer={
					<>
						<Button onClick={() => setConfirming(false)}>{t('common.cancel')}</Button>
						<Button
							variant="danger"
							loading={del.isPending}
							onClick={() => {
								del.mutate(user.id, {
									onSuccess: () => {
										setConfirming(false);
										pushToast(t('users.deleted'), 'info');
									}
								});
							}}
						>
							{t('common.delete')}
						</Button>
					</>
				}
			>
				<p className="settings-modal-text">
					{t('users.deleteUserConfirm', { username: user.username })}
				</p>
			</Modal>
		</div>
	);
}

export default function UsersPage() {
	const { t } = useTranslation();
	const whoami = useWhoami();
	const isAdmin = useIsAdmin();
	// No requests before the identity is known: a reader arriving directly is spared a list fetch doomed to 403
	const users = useUsers(isAdmin);
	const [creating, setCreating] = useState(false);
	const [resetting, setResetting] = useState<ManagedUser | null>(null);

	// A reader has no entries to click, so a direct URL visit bounces to the settings page (the server's 403 is the real gate)
	if (whoami.data && !isAdmin) {
		return <Navigate to="/settings" replace />;
	}

	return (
		<div className="page container settings-page">
			<PageHeader
				title={t('users.pageTitle')}
				actions={
					<Button variant="primary" onClick={() => setCreating(true)}>
						<UserPlus size={15} />
						{t('users.createTitle')}
					</Button>
				}
			/>
			{users.isPending ? (
				<div className="users-list">
					{Array.from({ length: 2 }, (_, i) => (
						<Skeleton key={i} height={64} style={{ borderRadius: 'var(--r-md)' }} />
					))}
				</div>
			) : (
				<div className="users-list">
					{(users.data ?? []).map((u) => (
						<UserRow
							key={u.id}
							user={u}
							selfId={whoami.data?.user?.id}
							onResetPassword={() => setResetting(u)}
						/>
					))}
				</div>
			)}

			<CreateUserModal open={creating} onClose={() => setCreating(false)} />
			<ResetPasswordModal user={resetting} onClose={() => setResetting(null)} />
		</div>
	);
}
