<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Settings from '@lucide/svelte/icons/settings';
	import { IconButton, Modal } from '$lib/components';
	import SourceSettingsPanel from './SourceSettingsPanel.svelte';

	interface Props {
		sourceId: number;
		sourceName: string;
	}
	let { sourceId, sourceName }: Props = $props();

	let open = $state(false);
</script>

<IconButton
	label={$_('browse.settingsTitle', { values: { name: sourceName } })}
	onclick={(e) => {
		e.stopPropagation();
		open = true;
	}}
>
	<Settings size={17} />
</IconButton>
<Modal
	{open}
	title={$_('browse.settingsTitle', { values: { name: sourceName } })}
	onClose={() => (open = false)}
	panelClassName="modal__panel--wide"
>
	{#if open}
		<SourceSettingsPanel {sourceId} />
	{/if}
</Modal>

<!--
  源设置弹窗 + SettingItem/BrowserLoginViewer 共用的样式。这些类被子组件的
  markup 使用，Svelte 作用域不跨组件，故用 :global 声明；本组件始终在 browse
  页包里，样式随之全局可用。对应 React 的 pages/settings.css 子集。
-->
<style>
	:global(.settings-source__body) {
		padding-top: 2px;
	}
	:global(.settings-source__loading) {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 0;
		color: var(--text-dim);
	}
	:global(.settings-source__empty) {
		margin: 8px 0 0;
	}
	:global(.settings-source__save) {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		margin-top: 8px;
	}

	:global(.setting-row) {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 8px 0;
	}
	:global(.setting-row--col) {
		flex-direction: column;
		align-items: stretch;
		gap: 6px;
	}
	:global(.setting-row--col .select),
	:global(.setting-row--col .select__field),
	:global(.setting-row--col .input),
	:global(.setting-row--col .input__field) {
		width: 100%;
	}
	:global(.setting-row--disabled) {
		opacity: 0.7;
	}
	:global(.setting-row--disabled .setting-row__title) {
		color: var(--text);
	}
	:global(.setting-row--disabled .faint) {
		font-size: 12.5px;
	}
	:global(.setting-row__title) {
		font-size: 14px;
		color: var(--text-dim);
	}
	:global(.setting-row__sub) {
		font-size: 12.5px;
		margin-top: 2px;
	}
	:global(.setting-login__head) {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	:global(.setting-group) {
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 10px 14px;
		margin: 8px 0;
	}
	:global(.setting-group__title) {
		font-weight: 600;
		font-size: 13.5px;
		color: var(--accent);
		margin-bottom: 4px;
		display: flex;
		align-items: center;
		gap: 6px;
	}
	:global(.setting-group__icon) {
		width: 20px;
		height: 20px;
		border-radius: 4px;
	}
	:global(.setting-group__info) {
		font-size: 12.5px;
		margin-bottom: 4px;
	}
	:global(.setting-group__footer) {
		font-size: 12.5px;
		margin-top: 6px;
	}

	:global(.setting-chips) {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	:global(.setting-ms) {
		display: flex;
		flex-direction: column;
	}
	:global(.setting-ms__row) {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 8px 0;
	}

	:global(.setting-link) {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		color: var(--accent);
		padding: 8px 0;
		font-size: 14px;
	}
	:global(.setting-link:hover) {
		color: var(--accent-hover);
	}

	:global(.setting-notification) {
		font-size: 12.5px;
		margin-top: -4px;
	}

	:global(.setting-textarea) {
		width: 100%;
		background: var(--bg-elev);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		padding: 8px 12px;
		font-size: 14px;
		color: var(--text);
		resize: vertical;
		transition: border-color 0.15s;
	}
	:global(.setting-textarea:focus-visible) {
		outline: none;
		border-color: var(--accent);
	}

	:global(.setting-stepper__input) {
		width: 110px;
		flex: none;
	}

	/* sidecar 交互登录的实时视图：16:10 远端视口等比缩放 */
	:global(.browser-view) {
		width: 100%;
		aspect-ratio: 16 / 10;
		background: #111;
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		overflow: hidden;
		outline: none;
	}
	:global(.browser-view:focus-visible) {
		border-color: var(--accent);
	}
	:global(.browser-view__screen) {
		width: 100%;
		height: 100%;
		object-fit: contain;
		display: block;
		user-select: none;
		cursor: default;
	}
	:global(.browser-view__placeholder) {
		display: flex;
		align-items: center;
		justify-content: center;
		height: 100%;
	}
</style>
