<script lang="ts">
	import { _ } from 'svelte-i18n';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import Section from './Section.svelte';
</script>

<Section title={$_('settings.aboutTitle')} desc={$_('settings.aboutDesc')}>
	<div class="settings-about">
		<p><strong>Sumidoku</strong> · {$_('settings.aboutTagline')}</p>
		<p class="muted">{$_('settings.aboutApis')}</p>
		<p class="faint">{$_('settings.aboutOpds')}</p>
		<a class="setting-link" href="https://github.com/Sumidoku/Sumidoku" target="_blank" rel="noreferrer">
			GitHub
			<ExternalLink size={13} />
		</a>
	</div>
</Section>
