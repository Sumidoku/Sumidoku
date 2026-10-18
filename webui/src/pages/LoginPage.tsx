import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { BookOpen } from 'lucide-react';
import { ApiError, api, setToken } from '../api/client';
import { Button, Input } from '../components';
import './login.css';

interface LoginResponse {
	token: string;
	expiresAt: number;
}

export default function LoginPage() {
	const navigate = useNavigate();
	const [password, setPassword] = useState('');
	const [pending, setPending] = useState(false);
	const [error, setError] = useState('');

	// 服务端没开认证时登录页无意义，直接回首页
	useEffect(() => {
		api<{ enabled: boolean }>('/auth/status', { silent: true })
			.then((s) => {
				if (!s.enabled) navigate('/', { replace: true });
			})
			.catch(() => {});
	}, [navigate]);

	async function submit() {
		if (!password || pending) return;
		setPending(true);
		setError('');
		try {
			const res = await api<LoginResponse>('/auth/login', {
				method: 'POST',
				body: { password },
				silent: true
			});
			setToken(res.token);
			navigate('/', { replace: true });
		} catch (e) {
			// 401 对外形状统一，不区分「没配密码」与「密码错」
			setError(e instanceof ApiError && e.status === 401 ? '密码错误' : '登录失败，请稍后重试');
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
				<p className="login-card__hint">此服务器已启用认证，请输入密码</p>
				<Input
					type="password"
					placeholder="密码"
					autoFocus
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				{error && <p className="login-card__error">{error}</p>}
				<Button variant="primary" type="submit" loading={pending} disabled={!password}>
					登录
				</Button>
			</form>
		</div>
	);
}
