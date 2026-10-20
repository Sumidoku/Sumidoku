<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { _ } from 'svelte-i18n';
	import BookOpen from '@lucide/svelte/icons/book-open';
	import { ApiError, api, setToken } from '$lib/api/client';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';

	interface LoginResponse {
		token: string;
		expiresAt: number;
	}

	let password = $state('');
	let pending = $state(false);
	let error = $state('');

	// 服务端没开认证时登录页无意义，直接回首页
	onMount(() => {
		api<{ enabled: boolean }>('/auth/status', { silent: true })
			.then((s) => {
				if (!s.enabled) goto('/', { replaceState: true });
			})
			.catch(() => {});
	});

	async function submit() {
		if (!password || pending) return;
		pending = true;
		error = '';
		try {
			const res = await api<LoginResponse>('/auth/login', {
				method: 'POST',
				body: { password },
				silent: true
			});
			setToken(res.token);
			goto('/', { replaceState: true });
		} catch (e) {
			// 401 对外形状统一，不区分「没配密码」与「密码错」
			error = e instanceof ApiError && e.status === 401 ? $_('login.errorWrong') : $_('login.errorGeneric');
		} finally {
			pending = false;
		}
	}
</script>

<div class="login-page">
	<form
		class="login-card"
		onsubmit={(e) => {
			e.preventDefault();
			void submit();
		}}
	>
		<div class="login-card__brand">
			<BookOpen size={28} />
			<h1>Sumidoku</h1>
		</div>
		<p class="login-card__hint">{$_('login.hint')}</p>
		<!-- svelte-ignore a11y_autofocus -->
		<Input type="password" placeholder={$_('login.password')} autofocus bind:value={password} />
		{#if error}
			<p class="login-card__error">{error}</p>
		{/if}
		<Button type="submit" disabled={!password || pending}>{$_('login.submit')}</Button>
	</form>
</div>

<style>
	.login-page {
		min-height: 100dvh;
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
	}

	.login-card {
		width: 100%;
		max-width: 340px;
		display: flex;
		flex-direction: column;
		gap: 14px;
		padding: 28px 24px;
		background: var(--bg-elev);
		border: 1px solid var(--border);
		border-radius: var(--r-lg);
		box-shadow: var(--shadow);
	}

	.login-card__brand {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 10px;
		color: var(--accent);
	}

	.login-card__brand h1 {
		font-size: 20px;
		color: var(--text);
	}

	.login-card__hint {
		text-align: center;
		font-size: 13px;
		color: var(--text-dim);
	}

	.login-card__error {
		text-align: center;
		font-size: 13px;
		color: var(--accent);
	}
</style>
