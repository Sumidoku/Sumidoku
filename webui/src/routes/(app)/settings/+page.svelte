<script lang="ts">
	import { _ } from 'svelte-i18n';
	import { PageHeader } from '$lib/components';
	import AppearanceSection from './AppearanceSection.svelte';
	import IncognitoSection from './IncognitoSection.svelte';
	import SecuritySection from './SecuritySection.svelte';
	import ApiKeysSection from './ApiKeysSection.svelte';
	import CategoriesSection from './CategoriesSection.svelte';
	import ServerSettingsSection from './ServerSettingsSection.svelte';
	import BackupSection from './BackupSection.svelte';
	import ScheduledBackupSection from './ScheduledBackupSection.svelte';
	import AboutSection from './AboutSection.svelte';
	import '$lib/styles/settings.css';
</script>

<div class="page container settings-page">
	<PageHeader title={$_('nav.settings')} />
	<AppearanceSection />
	<IncognitoSection />
	<SecuritySection />
	<ApiKeysSection />
	<CategoriesSection />
	<ServerSettingsSection />
	<BackupSection />
	<ScheduledBackupSection />
	<AboutSection />
</div>
