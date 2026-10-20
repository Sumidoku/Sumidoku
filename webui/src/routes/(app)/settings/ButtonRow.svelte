<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { Button } from '$lib/components';

	/** Button 设置：按下把 notification 发给源（Aidoku 语义）；没声明动作串时按下也是空操作，直接禁用 */
	interface Props {
		title: string;
		hasAction: boolean;
		onPress: () => Promise<void>;
	}
	let { title, hasAction, onPress }: Props = $props();

	let busy = $state(false);
</script>

<div class="setting-row">
	<Button
		size="sm"
		loading={busy}
		disabled={!hasAction}
		onclick={() => {
			busy = true;
			void onPress().finally(() => (busy = false));
		}}
	>
		{title}
	</Button>
	{#if !hasAction}<span class="faint">{$_('settings.buttonNoAction')}</span>{/if}
</div>
