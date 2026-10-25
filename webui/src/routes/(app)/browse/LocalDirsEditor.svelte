<script lang="ts">
	import { _ } from 'svelte-i18n';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { Button, IconButton, Input } from '$lib/components';

	interface Props {
		/** 受控的本地实例根目录列表：一行一个路径，可增删 */
		dirs: string[];
		onChange: (dirs: string[]) => void;
	}
	let { dirs, onChange }: Props = $props();
</script>

{#if dirs.length > 0}
	<div class="local-dirs">
		{#each dirs as dir, i (i)}
			<div class="local-dirs__row">
				<Input
					placeholder={$_('browse.localDirPlaceholder')}
					value={dir}
					oninput={(e) => {
						// 先取值再更新，避免事件对象在延迟更新时已被回收
						const v = e.currentTarget.value;
						onChange(dirs.map((d, j) => (j === i ? v : d)));
					}}
				/>
				<IconButton
					label={$_('common.delete')}
					variant="danger"
					onclick={() => onChange(dirs.filter((_d, j) => j !== i))}
				>
					<Trash2 size={16} />
				</IconButton>
			</div>
		{/each}
	</div>
{/if}
<div class="local-dirs__add">
	<Button onclick={() => onChange([...dirs, ''])}>{$_('browse.addLocalDir')}</Button>
</div>

<style>
	.local-dirs {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.local-dirs__row {
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.local-dirs__row :global(.input) {
		flex: 1;
	}
	.local-dirs__add {
		margin-top: 8px;
	}
</style>
