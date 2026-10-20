<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { Button, Chip, Input, SegmentedControl, Select, Switch } from '$lib/components';
	import type { Filter } from '$lib/api/types';
	import { optionKey, type CheckVal, type FilterStates } from './filterState';

	interface Props {
		filter: Filter;
		states: FilterStates;
		setStates: (s: FilterStates) => void;
		onCycleMulti: (f: Filter, i: number) => void;
	}
	let { filter: f, states, setStates, onCycleMulti }: Props = $props();

	const sortCur = $derived(states.sortVals[f.id] ?? { index: null, ascending: true });
	const checkCur = $derived(states.checkVals[f.id] ?? 0);
	const rangeCur = $derived(states.rangeVals[f.id] ?? { from: '', to: '' });
</script>

{#if f.type === 'note'}
	<div>
		<span class="filter-item__title">{f.title}</span>
		<p class="filter-item__note">{f.text}</p>
	</div>
{:else if f.type === 'text'}
	<div>
		<span class="filter-item__title">{f.title}</span>
		<Input
			type="text"
			placeholder={f.placeholder ?? ''}
			value={states.textVals[f.id] ?? ''}
			oninput={(e) =>
				setStates({
					...states,
					textVals: { ...states.textVals, [f.id]: e.currentTarget.value }
				})}
		/>
	</div>
{:else if f.type === 'sort'}
	<div>
		<span class="filter-item__title">{f.title}</span>
		<div class="filter-item__row">
			<Select
				value={sortCur.index === null ? '' : String(sortCur.index)}
				onchange={(e) => {
					const v = e.currentTarget.value;
					setStates({
						...states,
						sortVals: {
							...states.sortVals,
							[f.id]: { index: v === '' ? null : Number(v), ascending: sortCur.ascending }
						}
					});
				}}
			>
				<option value="">{$_('sourceBrowse.sortDefault')}</option>
				{#each f.options ?? [] as opt, i (i)}
					<option value={String(i)}>{opt}</option>
				{/each}
			</Select>
			{#if f.can_ascend && sortCur.index !== null}
				<Button
					onclick={() =>
						setStates({
							...states,
							sortVals: {
								...states.sortVals,
								[f.id]: { ...sortCur, ascending: !sortCur.ascending }
							}
						})}
				>
					{sortCur.ascending ? $_('sourceBrowse.ascending') : $_('sourceBrowse.descending')}
				</Button>
			{/if}
		</div>
	</div>
{:else if f.type === 'check'}
	{#if f.can_exclude}
		<div>
			<span class="filter-item__title">
				{f.title}{f.name ? ` · ${f.name}` : ''}
			</span>
			<SegmentedControl
				options={[
					{ value: 0, label: $_('sourceBrowse.checkNone') },
					{ value: 1, label: $_('sourceBrowse.checkInclude') },
					{ value: -1, label: $_('sourceBrowse.checkExclude') }
				]}
				value={checkCur}
				onChange={(v) =>
					setStates({ ...states, checkVals: { ...states.checkVals, [f.id]: v as CheckVal } })}
			/>
		</div>
	{:else}
		<div class="filter-item__row" style="justify-content: space-between">
			<span class="filter-item__title" style="margin-bottom: 0">
				{f.title}{f.name ? ` · ${f.name}` : ''}
			</span>
			<Switch
				checked={checkCur === 1}
				onChange={(on) =>
					setStates({ ...states, checkVals: { ...states.checkVals, [f.id]: on ? 1 : 0 } })}
				label={f.title ?? undefined}
			/>
		</div>
	{/if}
{:else if f.type === 'select'}
	<div>
		<span class="filter-item__title">{f.title}</span>
		<Select
			value={states.selectVals[f.id] ?? ''}
			onchange={(e) =>
				setStates({
					...states,
					selectVals: { ...states.selectVals, [f.id]: e.currentTarget.value }
				})}
		>
			<option value="">{$_('sourceBrowse.selectNone')}</option>
			{#each f.options ?? [] as opt, i (i)}
				<option value={optionKey(f, i)}>{opt}</option>
			{/each}
		</Select>
	</div>
{:else if f.type === 'multi-select'}
	<div>
		<span class="filter-item__title">{f.title}</span>
		<div class="filter-item__chips">
			{#each f.options ?? [] as opt, i (i)}
				{@const st = states.multiVals[f.id]?.[optionKey(f, i)] ?? 0}
				<Chip
					color={st === 1 ? 'accent' : st === -1 ? 'danger' : 'default'}
					title={f.can_exclude
						? $_('sourceBrowse.multiToggleHint')
						: $_('sourceBrowse.toggleHint')}
					onclick={() => onCycleMulti(f, i)}
				>
					{st === -1 ? '− ' : ''}{opt}
				</Chip>
			{/each}
		</div>
	</div>
{:else if f.type === 'range'}
	<div>
		<span class="filter-item__title">{f.title}</span>
		<div class="filter-item__row">
			<Input
				type="number"
				placeholder={f.min != null ? String(f.min) : $_('sourceBrowse.rangeMin')}
				step={f.decimal ? 'any' : '1'}
				value={rangeCur.from}
				oninput={(e) =>
					setStates({
						...states,
						rangeVals: {
							...states.rangeVals,
							[f.id]: { ...rangeCur, from: e.currentTarget.value }
						}
					})}
			/>
			<span class="faint">—</span>
			<Input
				type="number"
				placeholder={f.max != null ? String(f.max) : $_('sourceBrowse.rangeMax')}
				step={f.decimal ? 'any' : '1'}
				value={rangeCur.to}
				oninput={(e) =>
					setStates({
						...states,
						rangeVals: {
							...states.rangeVals,
							[f.id]: { ...rangeCur, to: e.currentTarget.value }
						}
					})}
			/>
		</div>
	</div>
{/if}
