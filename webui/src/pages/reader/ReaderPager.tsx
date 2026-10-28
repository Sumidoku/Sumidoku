import type { ChangeEvent } from 'react';
import { useTranslation } from 'react-i18next';
import type { ChapterWithDownload } from '../../api/types';
import { Button } from '../../components';
import type { Dir } from './direction';

interface ReaderPagerProps {
	dir: Dir;
	hidden: boolean;
	navPos: number;
	navTotal: number;
	prevCh: ChapterWithDownload | null;
	nextCh: ChapterWithDownload | null;
	onSlider: (e: ChangeEvent<HTMLInputElement>) => void;
	onNavChapter: (c: ChapterWithDownload | null) => void;
}

// Bottom bar of the reader. Under rtl the slider runs right-to-left (page 1 on the
// right) and the next-chapter button takes the leading left side, so every control
// agrees with the reading direction
export function ReaderPager({ dir, hidden, navPos, navTotal, prevCh, nextCh, onSlider, onNavChapter }: ReaderPagerProps) {
	const { t } = useTranslation();
	const prevButton = (
		<Button size="sm" disabled={!prevCh} onClick={() => onNavChapter(prevCh)}>
			{t('reader.prevChapter')}
		</Button>
	);
	const nextButton = (
		<Button size="sm" disabled={!nextCh} onClick={() => onNavChapter(nextCh)}>
			{t('reader.nextChapter')}
		</Button>
	);
	const pager = (
		<div className="reader__pager">
			<input
				type="range"
				min={1}
				max={navTotal}
				value={Math.min(navPos + 1, navTotal)}
				onChange={onSlider}
				aria-label={t('reader.jumpToPage')}
				style={dir === 'rtl' ? { direction: 'rtl' } : undefined}
			/>
			<span className="reader__page-num">
				{Math.min(navPos + 1, navTotal)}/{navTotal}
			</span>
		</div>
	);
	return (
		<div className={hidden ? 'reader__bar reader__bar--bottom reader__bar--hidden' : 'reader__bar reader__bar--bottom'}>
			{dir === 'rtl' ? (
				<>
					{nextButton}
					{pager}
					{prevButton}
				</>
			) : (
				<>
					{prevButton}
					{pager}
					{nextButton}
				</>
			)}
		</div>
	);
}
