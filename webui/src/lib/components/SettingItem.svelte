<script lang="ts">
	import { pushToast } from '#lib/toast.svelte';
	import type { Setting } from '#lib/types';
	import SettingItem from './SettingItem.svelte';

	let {
		setting,
		values,
		edits,
		onedit
	}: {
		setting: Setting;
		values: Record<string, unknown>;
		edits: Record<string, unknown>;
		onedit: (key: string, value: unknown) => void;
	} = $props();

	// 当前值优先取未保存的修改，其次取服务器已存值
	function cur(key: string): unknown {
		return key in edits ? edits[key] : values[key];
	}

	// requires / requires_false 控制显隐，按当前值实时判断
	let visible = $derived.by(() => {
		if (setting.requires && cur(setting.requires) !== true) return false;
		if (setting.requires_false && cur(setting.requires_false) === true) return false;
		return true;
	});

	let kind = $derived(Object.keys(setting.value)[0]);

	function strArr(v: unknown): string[] {
		return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [];
	}

	function num(v: unknown, fallback: number): number {
		return typeof v === 'number' && Number.isFinite(v) ? v : fallback;
	}
</script>

{#if visible}
	{#if 'Group' in setting.value}
		<div class="group">
			<div class="g-title">{setting.title}</div>
			{#each setting.value.Group.items as item (item.key)}
				<SettingItem setting={item} {values} {edits} {onedit} />
			{/each}
			{#if setting.value.Group.footer}
				<div class="faint" style="font-size: 12.5px; margin-top: 6px;">
					{setting.value.Group.footer}
				</div>
			{/if}
		</div>
	{:else if 'Page' in setting.value}
		<div class="group">
			<div class="g-title">{setting.title}</div>
			{#each setting.value.Page.items as item (item.key)}
				<SettingItem setting={item} {values} {edits} {onedit} />
			{/each}
		</div>
	{:else if 'Toggle' in setting.value}
		{@const v = setting.value.Toggle}
		<label class="row">
			<span>
				{setting.title}
				{#if v.subtitle}<div class="faint" style="font-size: 12.5px;">{v.subtitle}</div>{/if}
			</span>
			<input
				type="checkbox"
				class="switch"
				checked={(cur(setting.key) ?? v.default) === true}
				onchange={(e) => onedit(setting.key, e.currentTarget.checked)}
			/>
		</label>
	{:else if 'Select' in setting.value}
		{@const v = setting.value.Select}
		<label class="row col">
			<span class="s-title">{setting.title}</span>
			<select
				value={(cur(setting.key) as string) ?? v.default ?? ''}
				onchange={(e) => onedit(setting.key, e.currentTarget.value)}
			>
				{#each v.values as opt, i (i)}
					<option value={opt}>{v.titles?.[i] ?? opt}</option>
				{/each}
			</select>
		</label>
	{:else if 'MultiSelect' in setting.value}
		{@const v = setting.value.MultiSelect}
		{@const selected = strArr(cur(setting.key) ?? v.default ?? [])}
		<div class="row col">
			<span class="s-title">{setting.title}</span>
			<div class="chips">
				{#each v.values as opt, i (i)}
					{@const on = selected.includes(opt)}
					<button
						type="button"
						class="chip clickable"
						class:on
						onclick={() =>
							onedit(
								setting.key,
								on ? selected.filter((x) => x !== opt) : [...selected, opt]
							)}
					>
						{v.titles?.[i] ?? opt}
					</button>
				{/each}
			</div>
		</div>
	{:else if 'Text' in setting.value}
		{@const v = setting.value.Text}
		<label class="row col">
			<span class="s-title">{setting.title}</span>
			<input
				type={v.secure ? 'password' : 'text'}
				placeholder={v.placeholder ?? ''}
				value={(cur(setting.key) as string) ?? v.default ?? ''}
				oninput={(e) => onedit(setting.key, e.currentTarget.value)}
			/>
		</label>
	{:else if 'Stepper' in setting.value}
		{@const v = setting.value.Stepper}
		{@const step = v.step_value ?? 1}
		{@const cv = num(cur(setting.key) ?? v.default, v.minimum_value)}
		<div class="row">
			<span class="s-title">{setting.title}</span>
			<span class="stepper">
				<button
					type="button"
					disabled={cv - step < v.minimum_value}
					onclick={() => onedit(setting.key, cv - step)}>−</button
				>
				<span class="step-val">{cv}</span>
				<button
					type="button"
					disabled={cv + step > v.maximum_value}
					onclick={() => onedit(setting.key, cv + step)}>＋</button
				>
			</span>
		</div>
	{:else if 'Segment' in setting.value}
		{@const v = setting.value.Segment}
		{@const cv = num(cur(setting.key) ?? v.default, 0)}
		<div class="row col">
			<span class="s-title">{setting.title}</span>
			<div class="seg">
				{#each v.options as opt, i (i)}
					<button type="button" class="seg-btn" class:on={cv === i} onclick={() => onedit(setting.key, i)}>
						{opt}
					</button>
				{/each}
			</div>
		</div>
	{:else if 'Button' in setting.value}
		<button
			class="row linkrow"
			onclick={() => pushToast('该操作需要在客户端中执行', 'info')}
		>
			{setting.title}
		</button>
	{:else if 'Link' in setting.value}
		<a class="row linkrow" href={setting.value.Link.url} target="_blank" rel="noreferrer">
			{setting.title} ↗
		</a>
	{:else if 'Login' in setting.value}
		<div class="row faint">{setting.title}（登录请在客户端中完成）</div>
	{:else}
		<div class="row faint">{setting.title}（暂不支持的类型：{kind}）</div>
	{/if}

	{#if setting.notification}
		<div class="faint" style="font-size: 12.5px; margin-top: -4px;">{setting.notification}</div>
	{/if}
{/if}

<style>
	.row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 8px 0;
	}

	.row.col {
		flex-direction: column;
		align-items: stretch;
		gap: 6px;
	}

	.s-title {
		font-size: 14px;
		color: var(--text-dim);
	}

	.group {
		border: 1px solid var(--border);
		border-radius: 10px;
		padding: 10px 14px;
		margin: 8px 0;
	}

	.g-title {
		font-weight: 600;
		font-size: 13.5px;
		color: var(--accent);
		margin-bottom: 4px;
	}

	.linkrow {
		display: block;
		width: 100%;
		text-align: left;
		border: none;
		background: none;
		color: var(--accent);
		padding: 8px 0;
		cursor: pointer;
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	.switch {
		appearance: none;
		width: 42px;
		height: 24px;
		border-radius: 999px;
		background: var(--border);
		position: relative;
		cursor: pointer;
		border: none;
		padding: 0;
		flex: none;
		transition: background 0.15s;
	}

	.switch::after {
		content: '';
		position: absolute;
		top: 3px;
		left: 3px;
		width: 18px;
		height: 18px;
		border-radius: 50%;
		background: #fff;
		transition: transform 0.15s;
	}

	.switch:checked {
		background: var(--accent);
	}

	.switch:checked::after {
		transform: translateX(18px);
	}

	.stepper {
		display: inline-flex;
		align-items: center;
		gap: 10px;
	}

	.stepper button {
		width: 30px;
		height: 30px;
		padding: 0;
	}

	.step-val {
		min-width: 36px;
		text-align: center;
		font-variant-numeric: tabular-nums;
	}

	.seg {
		display: flex;
		border: 1px solid var(--border);
		border-radius: 8px;
		overflow: hidden;
	}

	.seg-btn {
		flex: 1;
		border: none;
		border-radius: 0;
		background: var(--bg);
		padding: 6px 10px;
		font-size: 13.5px;
		color: var(--text-dim);
	}

	.seg-btn.on {
		background: var(--accent);
		color: #14161c;
		font-weight: 600;
	}
</style>
