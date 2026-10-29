import { useTranslation } from 'react-i18next';
import type { Page } from '../../api/types';
import { SegmentedControl, Select } from '../../components';
import { FONT_SIZE_LIMITS, LINE_HEIGHT_LIMITS } from './typography';
import type { ReaderFont, ReaderTypography } from './typography';
import { pageText } from './pageText';
import type { Dir, Mode, PageScale, ReaderBackground, TapZoneLayout } from './readerPrefs';
import type { SpreadLayout } from './spreads';

interface ReaderSettingsProps {
	mode: Mode;
	dir: Dir;
	onDirChange: (dir: Dir) => void;
	layout: SpreadLayout;
	onLayoutChange: (layout: SpreadLayout) => void;
	tapZones: TapZoneLayout;
	onTapZonesChange: (layout: TapZoneLayout) => void;
	bg: ReaderBackground;
	onBgChange: (bg: ReaderBackground) => void;
	scale: PageScale;
	onScaleChange: (scale: PageScale) => void;
	split: boolean;
	onSplitChange: (split: boolean) => void;
	stretch: boolean;
	onStretchChange: (stretch: boolean) => void;
	currentPage: Page | null;
	typography: ReaderTypography;
	onTypographyChange: (patch: Partial<ReaderTypography>) => void;
}

export function ReaderSettings({
	mode,
	dir,
	onDirChange,
	layout,
	onLayoutChange,
	tapZones,
	onTapZonesChange,
	bg,
	onBgChange,
	scale,
	onScaleChange,
	split,
	onSplitChange,
	stretch,
	onStretchChange,
	currentPage,
	typography,
	onTypographyChange
}: ReaderSettingsProps) {
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
			<div className="reader__settings-title">
				{mode !== 'paged' ? t('reader.layoutPagedOnly') : t('reader.layout')}
			</div>
			<div className={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
				<SegmentedControl
					options={[
						{ value: 'single', label: t('reader.layoutSingle') },
						{ value: 'double', label: t('reader.layoutDouble') },
						{ value: 'doubleNoCover', label: t('reader.layoutDoubleNoCover') }
					]}
					value={layout}
					onChange={onLayoutChange}
				/>
			</div>
			<div className="reader__settings-title">
				{mode !== 'paged' ? t('reader.tapZonesPagedOnly') : t('reader.tapZones')}
			</div>
			<div className={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
				<Select
					aria-label={t('reader.tapZones')}
					value={tapZones}
					onChange={(e) => onTapZonesChange(e.currentTarget.value as TapZoneLayout)}
				>
					<option value="lr">{t('reader.tzLr')}</option>
					<option value="rl">{t('reader.tzRl')}</option>
					<option value="edge">{t('reader.tzEdge')}</option>
					<option value="kindle">{t('reader.tzKindle')}</option>
					<option value="off">{t('reader.tzOff')}</option>
				</Select>
			</div>
			<div className="reader__settings-title">
				{mode !== 'paged' ? t('reader.scalePagedOnly') : t('reader.pageScale')}
			</div>
			<div className={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
				<Select
					aria-label={t('reader.pageScale')}
					value={scale}
					onChange={(e) => onScaleChange(e.currentTarget.value as PageScale)}
				>
					<option value="screen">{t('reader.scaleScreen')}</option>
					<option value="width">{t('reader.scaleWidth')}</option>
					<option value="widthShrink">{t('reader.scaleWidthShrink')}</option>
					<option value="height">{t('reader.scaleHeight')}</option>
					<option value="original">{t('reader.scaleOriginal')}</option>
				</Select>
			</div>
			<div className={mode !== 'paged' || layout !== 'single' ? 'reader__settings-dir--off' : undefined}>
				<label className="reader__stretch">
					<input
						type="checkbox"
						checked={split}
						onChange={(e) => onSplitChange(e.currentTarget.checked)}
					/>
					<span>
						{mode !== 'paged'
							? t('reader.splitWidePagedOnly')
							: layout !== 'single'
								? t('reader.splitWideSingleOnly')
								: t('reader.splitWide')}
					</span>
				</label>
			</div>
			<div className={mode !== 'paged' ? 'reader__settings-dir--off' : undefined}>
				<label className="reader__stretch">
					<input
						type="checkbox"
						checked={stretch}
						onChange={(e) => onStretchChange(e.currentTarget.checked)}
					/>
					<span>{mode !== 'paged' ? t('reader.stretchPagedOnly') : t('reader.stretch')}</span>
				</label>
			</div>
			<div className="reader__settings-title">{t('reader.background')}</div>
			<SegmentedControl
				options={[
					{ value: 'dark', label: t('reader.bgDark') },
					{ value: 'gray', label: t('reader.bgGray') },
					{ value: 'light', label: t('reader.bgLight') }
				]}
				value={bg}
				onChange={onBgChange}
			/>
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
