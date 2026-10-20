<script lang="ts">
	interface Props {
		checked?: boolean;
		onChange?: (checked: boolean) => void;
		disabled?: boolean;
		label?: string;
	}
	let { checked = $bindable(false), onChange, disabled, label }: Props = $props();
</script>

<input
	type="checkbox"
	class="switch"
	bind:checked
	{disabled}
	aria-label={label}
	onchange={(e) => onChange?.(e.currentTarget.checked)}
/>
