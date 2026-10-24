import { useTranslation } from 'react-i18next';
import type { Page } from '../../api/types';
import { SegmentedControl, Select } from '../../components';
import { FONT_SIZE_LIMITS, LINE_HEIGHT_LIMITS } from './typography';
import type { ReaderFont, ReaderTypography } from './typography';
import { pageText } from './pageText';

export type Mode = 'paged' | 'continuous' | 'webtoon';
export type Dir = 'ltr' | 'rtl';

interface ReaderSettingsProps {
	mode: Mode;
	dir: Dir;
	onDirChange: (dir: Dir) => void;
	currentPage: Page | null;
	typography: ReaderTypography;
	onTypographyChange: (patch: Partial<ReaderTypography>) => void;
}

export function ReaderSettings({ mode, dir, onDirChange, currentPage, typography, onTypographyChange }: ReaderSettingsProps) {
	const { t } = useTranslation();
	const showTypography = pageText(currentPage) !== null;
	return (
		<div className="reader__settings card">
			<div className="reader__settings-title">
				{mode !== 'paged' ? t('reader.directionPagedOnly') : t('reader.direction')}
			</div>
			<div className={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
				<SegmentedControl
					options={[
						{ value: 'ltr', label: t('reader.ltr') },
						{ value: 'rtl', label: t('reader.rtl') }
					]}
					value={dir}
					onChange={onDirChange}
				/>
			</div>
			{showTypography && (
				<>
					<div className="reader__settings-title">{t('reader.typography')}</div>
					<div className="reader__typography">
						<Select
							aria-label={t('reader.fontFamily')}
							value={typography.font}
							onChange={(e) => onTypographyChange({ font: e.currentTarget.value as ReaderFont })}
						>
							<option value="default">{t('reader.fontDefault')}</option>
							<option value="sans">{t('reader.fontSans')}</option>
							<option value="serif">{t('reader.fontSerif')}</option>
							<option value="mono">{t('reader.fontMono')}</option>
						</Select>
						<label>
							<span>{t('reader.fontSize')}</span>
							<input
								type="range"
								min={FONT_SIZE_LIMITS.min}
								max={FONT_SIZE_LIMITS.max}
								step={FONT_SIZE_LIMITS.step}
								value={typography.fontSize}
								onChange={(e) => onTypographyChange({ fontSize: Number(e.currentTarget.value) })}
							/>
							<span>{Math.round(typography.fontSize * 100)}%</span>
						</label>
						<label>
							<span>{t('reader.lineHeight')}</span>
							<input
								type="range"
								min={LINE_HEIGHT_LIMITS.min}
								max={LINE_HEIGHT_LIMITS.max}
								step={LINE_HEIGHT_LIMITS.step}
								value={typography.lineHeight}
								onChange={(e) => onTypographyChange({ lineHeight: Number(e.currentTarget.value) })}
							/>
							<span>{typography.lineHeight.toFixed(1)}</span>
						</label>
					</div>
				</>
			)}
		</div>
	);
}
