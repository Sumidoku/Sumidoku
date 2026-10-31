import { useTranslation } from 'react-i18next';
import type { ChapterCacheSourceStats } from '../../api/types';
import { Button } from '../../components';
import { formatBytes } from '../../lib/format';

export interface ChapterCacheSourcesProps {
	sources: Array<ChapterCacheSourceStats>;
	clearing: boolean;
	onClear: (sourceId: number) => void;
}

export function ChapterCacheSources({ sources, clearing, onClear }: ChapterCacheSourcesProps) {
	const { t } = useTranslation();
	if (sources.length === 0) return null;
	return (
		<details className="settings-cache__sources">
			<summary>{t('settings.chapterCacheSources', { count: sources.length })}</summary>
			{sources.map((source) => (
				<div className="settings-row" key={source.sourceId}>
					<span className="settings-row__label">{source.sourceName}</span>
					<span className="faint">
						{t('settings.chapterCacheUsage', {
							used: formatBytes(source.usedBytes),
							count: source.chapterCount
						})}
					</span>
					<Button
						variant="danger"
						disabled={clearing}
						onClick={() => onClear(source.sourceId)}
					>
						{t('settings.clearSourceChapterCache')}
					</Button>
				</div>
			))}
		</details>
	);
}
