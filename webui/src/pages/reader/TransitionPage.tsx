import { useTranslation } from 'react-i18next';
import { Button } from '../../components';
import type { ChapterWithDownload } from '../../api/types';
import { chapterLabel } from '../../lib/format';

interface TransitionPageProps {
	/** 'next' = past the last page, 'prev' = before the first page */
	direction: 'next' | 'prev';
	/** Label of the chapter being left */
	currentLabel: string;
	/** Chapter the boundary turn would open */
	target: ChapterWithDownload | null;
	onContinue: () => void;
	onCancel: () => void;
}

// Shown when a turn crosses a chapter boundary, so reading never jumps chapters
// without a beat. Continue opens the target chapter; with no chapter left in
// that direction (target null) the page reports the end of the list and
// continue leaves the reader instead. Cancel stays on the page either way.
export function TransitionPage({ direction, currentLabel, target, onContinue, onCancel }: TransitionPageProps) {
	const { t } = useTranslation();
	const targetLabel = target ? chapterLabel(target) : '';
	const scanlator = target?.scanlators?.[0] ?? '';
	return (
		<div
			className="reader__transition"
			role="dialog"
			aria-modal="true"
			aria-label={t('reader.transitionTitle')}
			onClick={onCancel}
		>
			<div className="reader__transition-card" onClick={(e) => e.stopPropagation()}>
				<div className="reader__transition-state">
					{direction === 'next' ? t('reader.transitionFinished') : t('reader.transitionAtStart')}
				</div>
				<div className="reader__transition-current">{currentLabel}</div>
				{target ? (
					<div className="reader__transition-target">
						<div className="reader__transition-target-label">
							{direction === 'next' ? t('reader.transitionNext') : t('reader.transitionPrev')}
						</div>
						<div className="reader__transition-target-title">{targetLabel}</div>
						{scanlator && <div className="reader__transition-target-scan">{scanlator}</div>}
					</div>
				) : (
					<div className="reader__transition-target">
						<div className="reader__transition-target-title">
							{direction === 'next' ? t('reader.transitionNoNext') : t('reader.transitionNoPrev')}
						</div>
					</div>
				)}
				<div className="reader__transition-actions">
					<Button variant="primary" onClick={onContinue}>
						{target
							? direction === 'next'
								? t('reader.transitionContinue')
								: t('reader.transitionContinuePrev')
							: t('reader.transitionBackToManga')}
					</Button>
					<Button onClick={onCancel}>{t('reader.transitionStay')}</Button>
				</div>
			</div>
		</div>
	);
}
