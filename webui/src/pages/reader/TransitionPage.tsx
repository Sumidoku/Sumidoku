import { useTranslation } from 'react-i18next';
import type { RefObject } from 'react';
import type { ChapterWithDownload } from '../../api/types';
import { chapterLabel } from '../../lib/format';

interface TransitionPageProps {
	/** 'next' = past the last page, 'prev' = before the first page */
	direction: 'next' | 'prev';
	/** Label of the chapter being left */
	currentLabel: string;
	/** Chapter the boundary turn would open */
	target: ChapterWithDownload | null;
	/** Pull progress armed (next direction with a target only) */
	armed: boolean;
	/** Element the pull progress paints into (next direction with a target only) */
	hintRef?: RefObject<HTMLDivElement | null>;
}

// A page in the paged reader's track, parked one slot past either end of the
// chapter: it slides in with the ordinary turn animation instead of covering
// the page. There are no buttons — the tap zones own the whole viewport — so
// the confirm is another turn in the same direction (or a pull release on the
// next-side page) and the cancel is a turn back. With no chapter left in that
// direction the page reports the end of the list, and the confirming turn
// leaves the reader.
export function TransitionPage({ direction, currentLabel, target, armed, hintRef }: TransitionPageProps) {
	const { t } = useTranslation();
	const targetLabel = target ? chapterLabel(target) : '';
	const scanlator = target?.scanlators?.[0] ?? '';
	const hint = target
		? armed && direction === 'next'
			? t('reader.pullNextRelease', { chapter: target.chapter_number })
			: t('reader.transitionTurnToContinue')
		: t('reader.transitionExitHint');
	return (
		<div className="reader__transition" role="status" aria-live="polite" aria-label={t('reader.transitionTitle')}>
			<div className="reader__transition-state">
				{direction === 'next' ? t('reader.transitionFinished') : t('reader.transitionAtStart')}
			</div>
			<div className="reader__transition-current">{currentLabel}</div>
			<div className="reader__transition-divider" />
			<div className="reader__transition-target">
				{target ? (
					<>
						<div className="reader__transition-target-label">
							{direction === 'next' ? t('reader.transitionNext') : t('reader.transitionPrev')}
						</div>
						<div className="reader__transition-target-title">{targetLabel}</div>
						{scanlator && <div className="reader__transition-target-scan">{scanlator}</div>}
					</>
				) : (
					<div className="reader__transition-target-title">
						{direction === 'next' ? t('reader.transitionNoNext') : t('reader.transitionNoPrev')}
					</div>
				)}
			</div>
			<div className="reader__transition-hint">{hint}</div>
			{direction === 'next' && target && (
				<div className="reader__pull-track" ref={hintRef}>
					<div className="reader__pull-bar" />
				</div>
			)}
		</div>
	);
}
