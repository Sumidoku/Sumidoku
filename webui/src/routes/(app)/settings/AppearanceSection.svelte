<script lang="ts">
	import { _, locale } from 'svelte-i18n';
	import { browser } from '$app/env';
	import { theme, setTheme } from '$lib/theme';
	import type { Theme } from '$lib/theme';
	import { LOCALES, LOCALE_LABELS, setLocale } from '$lib/i18n';
	import type { Locale } from '$lib/i18n';
	import { SegmentedControl, Select } from '$lib/components';
	import Section from './Section.svelte';

	const READER_MODE_KEY = 'sumidoku.reader.mode';
	const READER_DIR_KEY = 'sumidoku.reader.dir';

	function stored(key: string, fallback: string): string {
		if (!browser) return fallback;
		try {
			return localStorage.getItem(key) ?? fallback;
		} catch {
			return fallback;
		}
	}

	// 旧版设置页把「滚动」存成了 vertical，阅读器只认 continuous，读入时归一
	let mode = $state(
		(() => {
			const m = stored(READER_MODE_KEY, 'paged');
			return m === 'vertical' ? 'continuous' : m;
		})()
	);
	let dir = $state(stored(READER_DIR_KEY, 'ltr'));

	function persist(key: string, value: string, apply: (v: string) => void) {
		apply(value);
		try {
			localStorage.setItem(key, value);
		} catch {
			// 隐私模式写不进去也照常切换，只是不持久化
		}
	}

	const themeOptions: { value: Theme; label: string }[] = $derived([
		{ value: 'system', label: $_('settings.themeSystem') },
		{ value: 'dark', label: $_('settings.themeDark') },
		{ value: 'light', label: $_('settings.themeLight') }
	]);
</script>

<Section title={$_('settings.appearanceTitle')} desc={$_('settings.appearanceDesc')}>
	<div class="settings-row">
		<span class="settings-row__label">{$_('settings.theme')}</span>
		<SegmentedControl options={themeOptions} value={$theme} onChange={(t) => setTheme(t)} />
	</div>
	<div class="settings-row">
		<span class="settings-row__label">{$_('locale.label')}</span>
		<Select
			value={$locale ?? 'en'}
			onchange={(e) => {
				const val = e.currentTarget.value as Locale;
				setLocale(val);
			}}
		>
			{#each LOCALES as l (l)}
				<option value={l}>{LOCALE_LABELS[l]}</option>
			{/each}
		</Select>
	</div>
	<div class="settings-row">
		<span class="settings-row__label">{$_('settings.readerModeLabel')}</span>
		<Select
			value={mode}
			onchange={(e) => {
				const val = e.currentTarget.value;
				persist(READER_MODE_KEY, val, (x) => (mode = x));
			}}
		>
			<option value="paged">{$_('settings.readerModePaged')}</option>
			<option value="continuous">{$_('settings.readerModeContinuous')}</option>
			<option value="webtoon">{$_('settings.readerModeWebtoon')}</option>
		</Select>
	</div>
	<div class="settings-row">
		<span class="settings-row__label">{$_('settings.readerDirLabel')}</span>
		<Select
			value={dir}
			onchange={(e) => {
				const val = e.currentTarget.value;
				persist(READER_DIR_KEY, val, (x) => (dir = x));
			}}
		>
			<option value="ltr">{$_('settings.readerDirLtr')}</option>
			<option value="rtl">{$_('settings.readerDirRtl')}</option>
		</Select>
	</div>
</Section>
