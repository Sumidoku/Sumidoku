<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import GripVertical from '@lucide/svelte/icons/grip-vertical';
	import House from '@lucide/svelte/icons/house';
	import Pin from '@lucide/svelte/icons/pin';
	import PinOff from '@lucide/svelte/icons/pin-off';
	import Trash from '@lucide/svelte/icons/trash';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import { sourceIconUrl } from '$lib/api/client';
	import type { Source } from '$lib/api/types';
	import { contentRatingColor, contentRatingLabel, languageLabel } from '$lib/source';
	import { Chip, IconButton } from '$lib/components';
	import SourceIcon from './SourceIcon.svelte';
	import UpdateButton from './UpdateButton.svelte';
	import SourceSettingsModal from './SourceSettingsModal.svelte';

	/** 置顶段卡片开启拖拽排序时传入；普通列表不传 */
	export interface CardDnd {
		dragging: boolean;
		dragOver: boolean;
		onDragStart: (e: DragEvent) => void;
		onDragOver: (e: DragEvent) => void;
		onDrop: () => void;
		onDragEnd: () => void;
	}

	interface Props {
		source: Source;
		dnd?: CardDnd;
		onTogglePin: (s: Source) => void;
		onDelete: (s: Source) => void;
	}
	let { source: s, dnd, onTogglePin, onDelete }: Props = $props();
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
<div
	class={[
		'source-card',
		s.enabled ? 'source-card--link' : 'source-card--disabled',
		dnd?.dragging && 'source-card--dragging',
		dnd?.dragOver && 'source-card--drag-over'
	]}
	role={s.enabled ? 'link' : undefined}
	tabindex={s.enabled ? 0 : undefined}
	onclick={() => s.enabled && goto(`/browse/${s.id}`)}
	onkeydown={(e) => {
		if (e.target !== e.currentTarget) return;
		if (s.enabled && (e.key === 'Enter' || e.key === ' ')) {
			e.preventDefault();
			goto(`/browse/${s.id}`);
		}
	}}
	ondragover={dnd?.onDragOver}
	ondrop={dnd?.onDrop}
>
	{#if dnd}
		<!-- 只让手柄可拖：整卡 draggable 会把点按导航也变成拖拽起手 -->
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<span
			class="source-card__grip"
			draggable="true"
			ondragstart={(e) => dnd.onDragStart(e)}
			ondragend={() => dnd.onDragEnd()}
		>
			<GripVertical size={16} />
		</span>
	{/if}
	<SourceIcon src={sourceIconUrl(s.id)} name={s.name} fallbackKey={s.key} />
	<div class="source-card__main">
		<div class="source-card__name">
			<span class="source-card__name-text">{s.name}</span>
			{#if s.contentRating > 0}
				<Chip color={contentRatingColor(s.contentRating)}>
					{contentRatingLabel(s.contentRating)}
				</Chip>
			{/if}
		</div>
		<div class="source-card__meta">
			{s.key} · v{s.version}{!s.enabled ? ` · ${$_('browse.disabled')}` : ''}
			{#if s.missingFromRepos}
				<span class="source-card__missing" title={$_('browse.missingFromRepos')}>
					<TriangleAlert size={13} />
				</span>
			{/if}
		</div>
		{#if s.languages.length > 0}
			<div class="source-card__badges">
				{#each s.languages.slice(0, 3) as lang (lang)}
					<Chip>{languageLabel(lang)}</Chip>
				{/each}
				{#if s.languages.length > 3}<Chip>+{s.languages.length - 3}</Chip>{/if}
			</div>
		{/if}
	</div>
	{#if s.updateVersion !== null}
		<UpdateButton source={s} />
	{/if}
	<IconButton
		label={s.pinned
			? $_('browse.unpinLabel', { values: { name: s.name } })
			: $_('browse.pinLabel', { values: { name: s.name } })}
		onclick={(e) => {
			e.stopPropagation();
			onTogglePin(s);
		}}
	>
		{#if s.pinned}<PinOff size={17} />{:else}<Pin size={17} />{/if}
	</IconButton>
	<IconButton
		label={$_('browse.homeLabel', { values: { name: s.name } })}
		onclick={(e) => {
			e.stopPropagation();
			goto(`/browse/${s.id}/home`);
		}}
	>
		<House size={17} />
	</IconButton>
	<SourceSettingsModal sourceId={s.id} sourceName={s.name} />
	<IconButton
		label={$_('browse.uninstallLabel', { values: { name: s.name } })}
		variant="danger"
		onclick={(e) => {
			e.stopPropagation();
			onDelete(s);
		}}
	>
		<Trash size={17} />
	</IconButton>
</div>
