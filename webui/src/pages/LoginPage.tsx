import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { BookOpen } from 'lucide-react';
import { ApiError, api } from '../api/client';
import { qk, queryClient } from '../api/hooks';
import type { AuthStatus, LoginResponse } from '../api/types';
import { Button, Input } from '../components';
import './login.css';

export default function LoginPage() {
	const navigate = useNavigate();
	const { t } = useTranslation();
	const [username, setUsername] = useState('');
	const [password, setPassword] = useState('');
	const [pending, setPending] = useState(false);
	const [error, setError] = useState('');

	// The login page is pointless when the server has auth disabled, so bounce straight home
	useEffect(() => {
		api<AuthStatus>('/auth/status', { silent: true })
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
			// The login response carries the identity: write it straight into the cache so whoami doesn't answer null for a beat after entering the home page
			queryClient.setQueryData(qk.whoami, { enabled: true, username: res.user.username, user: res.user });
			navigate('/', { replace: true });
		} catch (e) {
			// The 401 shape is uniform to the outside, not distinguishing "user does not exist" from "wrong password"
			setError(
				e instanceof ApiError && e.status === 401
					? t('login.error_invalid')
					: e instanceof ApiError && e.status === 429
						? t('login.error_throttled')
						: t('login.error_generic')
			);
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
				<p className="login-card__hint">{t('login.hint')}</p>
				<Input
					placeholder={t('login.username')}
					autoFocus
					autoComplete="username"
					value={username}
					onChange={(e) => setUsername(e.currentTarget.value)}
				/>
				<Input
					type="password"
					placeholder={t('login.password')}
					autoComplete="current-password"
					value={password}
					onChange={(e) => setPassword(e.currentTarget.value)}
				/>
				{error && <p className="login-card__error">{error}</p>}
				<Button variant="primary" type="submit" loading={pending} disabled={!username || !password}>
					{t('login.submit')}
				</Button>
			</form>
		</div>
	);
}
