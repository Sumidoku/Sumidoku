<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Dialog } from 'bits-ui';
	import { _ } from 'svelte-i18n';
	import X from '@lucide/svelte/icons/x';
	import IconButton from './IconButton.svelte';

	interface Props {
		open: boolean;
		title: string;
		onClose: () => void;
		children: Snippet;
		footer?: Snippet;
		/** 追加到面板的类名，用来放宽宽度等 */
		panelClassName?: string;
	}
	let { open, title, onClose, children, footer, panelClassName = '' }: Props = $props();
</script>

<Dialog.Root
	{open}
	onOpenChange={(o) => {
		if (!o) onClose();
	}}
>
	<Dialog.Portal>
		<Dialog.Overlay class="modal__mask" />
		<Dialog.Content class={['modal__panel', panelClassName]}>
			<div class="modal__head">
				<Dialog.Title>
					{#snippet child({ props })}
						<h3 {...props}>{title}</h3>
					{/snippet}
				</Dialog.Title>
				<IconButton label={$_('common.close')} size="sm" onclick={onClose}>
					<X size={16} />
				</IconButton>
			</div>
			<div class="modal__body">{@render children()}</div>
			{#if footer}<div class="modal__footer">{@render footer()}</div>{/if}
		</Dialog.Content>
	</Dialog.Portal>
</Dialog.Root>
