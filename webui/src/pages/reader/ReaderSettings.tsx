import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { Page } from '../../api/types';
import { SegmentedControl, Select, Switch } from '../../components';
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
	anim: boolean;
	onAnimChange: (anim: boolean) => void;
	transition: boolean;
	onTransitionChange: (transition: boolean) => void;
	currentPage: Page | null;
	typography: ReaderTypography;
	onTypographyChange: (patch: Partial<ReaderTypography>) => void;
}

/** A titled block of the menu; a note marks it unavailable in the current mode and locks its controls */
function SettingsGroup({ title, note, children }: { title: string; note?: string; children: ReactNode }) {
	return (
		<section className="reader__settings-group">
			<div className="reader__settings-head">
				<h3 className="reader__settings-title">{title}</h3>
				{note && <span>{note}</span>}
			</div>
			<div className={note ? 'reader__settings-body reader__settings-body--off' : 'reader__settings-body'} inert={!!note}>
				{children}
			</div>
		</section>
	);
}

function SwitchRow({
	label,
	checked,
	onChange,
	disabled
}: {
	label: string;
	checked: boolean;
	onChange: (checked: boolean) => void;
	disabled?: boolean;
}) {
	return (
		<label className={disabled ? 'reader__settings-row reader__settings-row--off' : 'reader__settings-row'}>
			<span>{label}</span>
			<Switch checked={checked} onChange={onChange} disabled={disabled} label={label} />
		</label>
	);
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
	anim,
	onAnimChange,
	transition,
	onTransitionChange,
	currentPage,
	typography,
	onTypographyChange
}: ReaderSettingsProps) {
	const { t } = useTranslation();
	const showTypography = pageText(currentPage) !== null;
	const pagedOnly = mode !== 'paged';
	const splitAvailable = layout === 'single';
	return (
		<div className="reader__settings card">
			<SettingsGroup title={t('reader.groupPage')} note={pagedOnly ? t('reader.pagedOnly') : undefined}>
				<SegmentedControl
					aria-label={t('reader.layout')}
					options={[
						{ value: 'single', label: t('reader.layoutSingle') },
						{ value: 'double', label: t('reader.layoutDouble') },
						{ value: 'doubleNoCover', label: t('reader.layoutDoubleNoCover') }
					]}
					value={layout}
					onChange={onLayoutChange}
				/>
				<div className="reader__settings-row">
					<span>{t('reader.pageScale')}</span>
					<Select
						aria-label={t('reader.pageScale')}
						value={scale}
						onChange={(value) => onScaleChange(value as PageScale)}
					>
						<option value="screen">{t('reader.scaleScreen')}</option>
						<option value="width">{t('reader.scaleWidth')}</option>
						<option value="widthShrink">{t('reader.scaleWidthShrink')}</option>
						<option value="height">{t('reader.scaleHeight')}</option>
						<option value="original">{t('reader.scaleOriginal')}</option>
					</Select>
				</div>
				<SwitchRow
					label={splitAvailable ? t('reader.splitWide') : t('reader.splitWideSingleOnly')}
					checked={split}
					onChange={onSplitChange}
					disabled={!splitAvailable}
				/>
				<SwitchRow label={t('reader.stretch')} checked={stretch} onChange={onStretchChange} />
			</SettingsGroup>
			<SettingsGroup title={t('reader.groupTurning')} note={pagedOnly ? t('reader.pagedOnly') : undefined}>
				<SegmentedControl
					aria-label={t('reader.direction')}
					options={[
						{ value: 'ltr', label: t('reader.ltr') },
						{ value: 'rtl', label: t('reader.rtl') }
					]}
					value={dir}
					onChange={onDirChange}
				/>
				<div className="reader__settings-row">
					<span>{t('reader.tapZones')}</span>
					<Select
						aria-label={t('reader.tapZones')}
						value={tapZones}
						onChange={(value) => onTapZonesChange(value as TapZoneLayout)}
					>
						<option value="lr">{t('reader.tzLr')}</option>
						<option value="rl">{t('reader.tzRl')}</option>
						<option value="edge">{t('reader.tzEdge')}</option>
						<option value="kindle">{t('reader.tzKindle')}</option>
						<option value="off">{t('reader.tzOff')}</option>
					</Select>
				</div>
				<SwitchRow label={t('reader.animations')} checked={anim} onChange={onAnimChange} />
				<SwitchRow label={t('reader.transition')} checked={transition} onChange={onTransitionChange} />
			</SettingsGroup>
			<SettingsGroup title={t('reader.groupAppearance')}>
				<SegmentedControl
					aria-label={t('reader.background')}
					options={[
						{ value: 'dark', label: t('reader.bgDark') },
						{ value: 'gray', label: t('reader.bgGray') },
						{ value: 'light', label: t('reader.bgLight') },
						{ value: 'auto', label: t('reader.bgAuto') }
					]}
					value={bg}
					onChange={onBgChange}
				/>
			</SettingsGroup>
			{showTypography && (
				<SettingsGroup title={t('reader.typography')}>
					<div className="reader__typography">
						<Select
							aria-label={t('reader.fontFamily')}
							value={typography.font}
							onChange={(value) => onTypographyChange({ font: value as ReaderFont })}
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
				</SettingsGroup>
			)}
		</div>
	);
}
