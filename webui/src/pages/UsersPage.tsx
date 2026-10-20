import { useState } from 'react';
import { Navigate } from 'react-router-dom';
import { KeyRound, Trash2, UserPlus } from 'lucide-react';
import { ApiError } from '../api/client';
import { useCreateUser, useDeleteUser, useIsAdmin, useUpdateUser, useUsers, useWhoami } from '../api/hooks';
import type { ManagedUser } from '../api/types';
import { Badge, Button, IconButton, Input, Modal, PageHeader, Select, Skeleton, pushToast } from '../components';
import './users.css';

const ROLE_LABEL: Record<string, string> = { admin: '管理员', reader: '读者' };

function toastError(e: unknown) {
	if (e instanceof ApiError && e.status === 409) pushToast('用户名已存在');
	else if (e instanceof ApiError && e.status === 400) pushToast(e.message);
	else pushToast('操作失败');
}

function CreateUserModal({ open, onClose }: { open: boolean; onClose: () => void }) {
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
			pushToast(`已创建用户 ${username.trim()}`, 'info');
			reset();
			onClose();
		} catch (e) {
			toastError(e);
		}
	}

	return (
		<Modal
			open={open}
			title="新建用户"
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>取消</Button>
					<Button
						variant="primary"
						loading={create.isPending}
						disabled={!username.trim() || !password}
						onClick={() => void submit()}
					>
						创建
					</Button>
				</>
			}
		>
			<div className="users-form">
				<Input
					placeholder="用户名"
					value={username}
					onChange={(e) => setUsername(e.currentTarget.value)}
				/>
				<Input
					type="password"
					placeholder="初始密码"
					autoComplete="new-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				<Select value={role} onChange={(e) => setRole(e.currentTarget.value)}>
					<option value="reader">读者：只有自己的书库、进度与分类</option>
					<option value="admin">管理员：可管理用户、源与服务器设置</option>
				</Select>
			</div>
		</Modal>
	);
}

function ResetPasswordModal({ user, onClose }: { user: ManagedUser | null; onClose: () => void }) {
	const update = useUpdateUser();
	const [password, setPassword] = useState('');

	async function submit() {
		if (!user) return;
		try {
			await update.mutateAsync({ id: user.id, password });
			pushToast(`已重置 ${user.username} 的密码，其全部会话已下线`, 'info');
			setPassword('');
			onClose();
		} catch (e) {
			toastError(e);
		}
	}

	return (
		<Modal
			open={user !== null}
			title={`重置密码：${user?.username ?? ''}`}
			onClose={onClose}
			footer={
				<>
					<Button onClick={onClose}>取消</Button>
					<Button
						variant="primary"
						loading={update.isPending}
						disabled={!password}
						onClick={() => void submit()}
					>
						重置
					</Button>
				</>
			}
		>
			<div className="users-form">
				<Input
					type="password"
					placeholder="新密码"
					autoComplete="new-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				<p className="faint">重置后该用户的所有登录会话立即失效。</p>
			</div>
		</Modal>
	);
}

function UserRow({ user, selfId, onResetPassword }: { user: ManagedUser; selfId?: number; onResetPassword: () => void }) {
	const update = useUpdateUser();
	const del = useDeleteUser();
	const [confirming, setConfirming] = useState(false);
	// user 1 是内置 admin：服务端拒删（400），入口直接不给
	const builtin = user.id === 1;

	async function changeRole(role: string) {
		try {
			await update.mutateAsync({ id: user.id, role });
			pushToast(`已把 ${user.username} 改为${ROLE_LABEL[role] ?? role}`, 'info');
		} catch (e) {
			toastError(e);
		}
	}

	return (
		<div className="users-row">
			<div className="users-row__main">
				<span className="users-row__name">
					{user.username}
					{user.id === selfId && <Badge>当前账号</Badge>}
				</span>
				<span className="faint">
					{ROLE_LABEL[user.role] ?? user.role} · 创建于 {new Date(user.createdAt * 1000).toLocaleString()}
				</span>
			</div>
			<div className="users-row__ops">
				<Select
					value={user.role}
					disabled={update.isPending}
					onChange={(e) => void changeRole(e.currentTarget.value)}
				>
					<option value="admin">管理员</option>
					<option value="reader">读者</option>
				</Select>
				<IconButton label={`重置 ${user.username} 的密码`} onClick={onResetPassword}>
					<KeyRound size={16} />
				</IconButton>
				{!builtin && (
					<IconButton label={`删除 ${user.username}`} variant="danger" onClick={() => setConfirming(true)}>
						<Trash2 size={16} />
					</IconButton>
				)}
			</div>

			<Modal
				open={confirming}
				title={`删除用户 ${user.username}？`}
				onClose={() => setConfirming(false)}
				footer={
					<>
						<Button onClick={() => setConfirming(false)}>取消</Button>
						<Button
							variant="danger"
							loading={del.isPending}
							onClick={() => {
								del.mutate(user.id, {
									onSuccess: () => {
										setConfirming(false);
										pushToast('已删除', 'info');
									}
								});
							}}
						>
							删除
						</Button>
					</>
				}
			>
				<p className="settings-modal-text">
					删除后 {user.username} 的书库、阅读进度、历史、分类与下载记录一并清除，此操作不可撤销。
				</p>
			</Modal>
		</div>
	);
}

export default function UsersPage() {
	const whoami = useWhoami();
	const isAdmin = useIsAdmin();
	// 身份未确认前不发请求：reader 直达时省一次注定 403 的列表拉取
	const users = useUsers(isAdmin);
	const [creating, setCreating] = useState(false);
	const [resetting, setResetting] = useState<ManagedUser | null>(null);

	// reader 没有入口可点，直达 URL 时回设置页（服务端 403 是真门禁）
	if (whoami.data && !isAdmin) {
		return <Navigate to="/settings" replace />;
	}

	return (
		<div className="page container settings-page">
			<PageHeader
				title="用户管理"
				actions={
					<Button variant="primary" onClick={() => setCreating(true)}>
						<UserPlus size={15} />
						新建用户
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
