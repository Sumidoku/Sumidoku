import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { BookOpen } from 'lucide-react';
import { ApiError, api, setToken } from '../api/client';
import { qk, queryClient } from '../api/hooks';
import type { AccountUser } from '../api/types';
import { Button, Input } from '../components';
import './login.css';

interface LoginResponse {
	token: string;
	expiresAt: number;
	user: AccountUser;
}

export default function LoginPage() {
	const navigate = useNavigate();
	const [username, setUsername] = useState('');
	const [password, setPassword] = useState('');
	const [pending, setPending] = useState(false);
	const [error, setError] = useState('');

	// The login page is pointless when the server has auth disabled, so bounce straight home
	useEffect(() => {
		api<{ enabled: boolean }>('/auth/status', { silent: true })
			.then((s) => {
				if (!s.enabled) navigate('/', { replace: true });
			})
			.catch(() => {});
	}, [navigate]);

	async function submit() {
		if (!username || !password || pending) return;
		setPending(true);
		setError('');
		try {
			const res = await api<LoginResponse>('/auth/login', {
				method: 'POST',
				body: { username, password },
				silent: true
			});
			setToken(res.token);
			// The login response carries the identity: write it straight into the cache so whoami doesn't answer null for a beat after entering the home page
			queryClient.setQueryData(qk.whoami, { enabled: true, user: res.user });
			navigate('/', { replace: true });
		} catch (e) {
			// The 401 shape is uniform to the outside, not distinguishing "user does not exist" from "wrong password"
			setError(e instanceof ApiError && e.status === 401 ? '用户名或密码错误' : '登录失败，请稍后重试');
		} finally {
			setPending(false);
		}
	}

	return (
		<div className="login-page">
			<form
				className="login-card card"
				onSubmit={(e) => {
					e.preventDefault();
					void submit();
				}}
			>
				<div className="login-card__brand">
					<BookOpen size={28} />
					<h1>Sumidoku</h1>
				</div>
				<p className="login-card__hint">此服务器已启用认证，请输入用户名和密码</p>
				<Input
					placeholder="用户名"
					autoFocus
					autoComplete="username"
					value={username}
					onChange={(e) => setUsername(e.currentTarget.value)}
				/>
				<Input
					type="password"
					placeholder="密码"
					autoComplete="current-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				{error && <p className="login-card__error">{error}</p>}
				<Button variant="primary" type="submit" loading={pending} disabled={!username || !password}>
					登录
				</Button>
			</form>
		</div>
	);
}
