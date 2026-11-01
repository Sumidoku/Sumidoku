import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
	ArrowLeft,
	ArrowRight,
	Minimize2,
	BookOpen,
	Contrast,
	FileImage,
	Frame,
	Scan,
	Square,
	StretchHorizontal,
	StretchVertical
} from 'lucide-react';
import type { Page } from '../../api/types';
import { Select, Switch } from '../../components';
import { FONT_SIZE_LIMITS, LINE_HEIGHT_LIMITS } from './typography';
import type { ReaderFont, ReaderTypography } from './typography';
import { pageText } from './pageText';
import type { Dir, Mode, PageScale, ReaderBackground, TapZoneLayout, ZoneAction } from './readerPrefs';
import type { SpreadLayout } from './spreads';
import type { PageTurnStyle } from './pageTurn';
import type { ContinuousScale } from './continuousPrefs';
import { OptionTiles } from './OptionTiles';
import { TapZonePreview } from './TapZonePreview';

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
	pageTurn: PageTurnStyle;
	onPageTurnChange: (style: PageTurnStyle) => void;
	continuousScale: ContinuousScale;
	onContinuousScaleChange: (scale: ContinuousScale) => void;
	continuousPadding: number;
	onContinuousPaddingChange: (padding: number) => void;
	continuousGap: number;
	onContinuousGapChange: (gap: number) => void;
	transition: boolean;
	onTransitionChange: (transition: boolean) => void;
	currentPage: Page | null;
	typography: ReaderTypography;
	onTypographyChange: (patch: Partial<ReaderTypography>) => void;
}

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
	pageTurn,
	onPageTurnChange,
	continuousScale,
	onContinuousScaleChange,
	continuousPadding,
	onContinuousPaddingChange,
	continuousGap,
	onContinuousGapChange,
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
		<div className="reader__settings card" data-theme="dark">
			<SettingsGroup title={t('reader.groupPage')} note={pagedOnly ? t('reader.pagedOnly') : undefined}>
				<OptionTiles
					label={t('reader.layout')}
					options={[
						{ value: 'single', label: t('reader.layoutSingle'), icon: <Square size={22} /> },
						{ value: 'double', label: t('reader.layoutDouble'), icon: <BookOpen size={22} /> },
						{ value: 'doubleNoCover', label: t('reader.layoutDoubleNoCover'), icon: <BookOpen size={22} /> }
					]}
					value={layout}
					onChange={onLayoutChange}
				/>
				<OptionTiles
					label={t('reader.pageScale')}
					options={[
						{ value: 'screen', label: t('reader.scaleScreen'), icon: <Scan size={22} /> },
						{ value: 'width', label: t('reader.scaleWidth'), icon: <StretchHorizontal size={22} /> },
						{ value: 'widthShrink', label: t('reader.scaleWidthShrink'), icon: <Minimize2 size={22} /> },
						{ value: 'height', label: t('reader.scaleHeight'), icon: <StretchVertical size={22} /> },
						{ value: 'original', label: t('reader.scaleOriginal'), icon: <Frame size={22} /> }
					]}
					value={scale}
					onChange={onScaleChange}
				/>
				<div className="reader__settings-card">
					<SwitchRow
						label={splitAvailable ? t('reader.splitWide') : t('reader.splitWideSingleOnly')}
						checked={split}
						onChange={onSplitChange}
						disabled={!splitAvailable}
					/>
					<SwitchRow label={t('reader.stretch')} checked={stretch} onChange={onStretchChange} />
				</div>
			</SettingsGroup>
			<SettingsGroup title={t('reader.groupTurning')} note={pagedOnly ? t('reader.pagedOnly') : undefined}>
				<OptionTiles
					label={t('reader.direction')}
					options={[
						{ value: 'ltr', label: t('reader.ltr'), icon: <ArrowRight size={22} /> },
						{ value: 'rtl', label: t('reader.rtl'), icon: <ArrowLeft size={22} /> }
					]}
					value={dir}
					onChange={onDirChange}
					columns={2}
				/>
				<div>
					<OptionTiles
						label={t('reader.tapZones')}
						options={(['lr', 'rl', 'edge', 'kindle', 'off'] as TapZoneLayout[]).map((value) => ({
							value,
							label: t(`reader.tz${value[0].toUpperCase()}${value.slice(1)}`),
							preview: <TapZonePreview layout={value} dir={dir} />
						}))}
						value={tapZones}
						onChange={onTapZonesChange}
					/>
					<div className="reader__tz-legend">
						{(['prev', 'next', 'menu'] as ZoneAction[]).map((action) => (
							<span key={action}><i className={`reader__tz-fill--${action}`} />{t(`reader.tzAction.${action}`)}</span>
						))}
					</div>
				</div>
				<div className="reader__settings-card">
					<SwitchRow label={t('reader.animations')} checked={anim} onChange={onAnimChange} />
					{anim && (
						<div className="reader__turn-style">
							<span>{t('reader.pageTurn')}</span>
							<OptionTiles
								label={t('reader.pageTurn')}
								options={[
									{ value: 'auto', label: t('reader.turnAuto') },
									{ value: 'slide', label: t('reader.turnSlide') },
									{ value: 'fade', label: t('reader.turnFade') }
								]}
								value={pageTurn}
								onChange={onPageTurnChange}
							/>
						</div>
					)}
					<SwitchRow label={t('reader.transition')} checked={transition} onChange={onTransitionChange} />
				</div>
			</SettingsGroup>
			{pagedOnly && (
				<SettingsGroup title={t('reader.groupContinuous')}>
					<OptionTiles
						label={t('reader.pageScale')}
						options={[
							{ value: 'width', label: t('reader.scaleWidth'), icon: <StretchHorizontal size={22} /> },
							{ value: 'original', label: t('reader.scaleOriginal'), icon: <FileImage size={22} /> }
						]}
						value={continuousScale}
						onChange={onContinuousScaleChange}
						columns={2}
					/>
					<div className="reader__settings-card reader__sliders">
						<label>
							<span>{t('reader.sidePadding')}</span>
							<input type="range" min={0} max={40} step={5} value={continuousPadding} onChange={(e) => onContinuousPaddingChange(Number(e.currentTarget.value))} />
							<output>{continuousPadding}%</output>
						</label>
						<label>
							<span>{t('reader.pageGap')}</span>
							<input type="range" min={0} max={40} step={5} value={continuousGap} onChange={(e) => onContinuousGapChange(Number(e.currentTarget.value))} />
							<output>{continuousGap}px</output>
						</label>
					</div>
				</SettingsGroup>
			)}
			<SettingsGroup title={t('reader.groupAppearance')}>
				<div className="reader__backgrounds" role="group" aria-label={t('reader.background')}>
					{(['auto', 'dark', 'gray', 'light'] as ReaderBackground[]).map((value) => (
						<button
							type="button"
							key={value}
							className={value === bg ? `reader__background reader__background--${value} reader__background--selected` : `reader__background reader__background--${value}`}
							aria-label={t(`reader.bg${value[0].toUpperCase()}${value.slice(1)}`)}
							aria-pressed={value === bg}
							title={t(`reader.bg${value[0].toUpperCase()}${value.slice(1)}`)}
							onClick={() => onBgChange(value)}
						>
							{value === 'auto' && <Contrast size={24} />}
						</button>
					))}
				</div>
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
