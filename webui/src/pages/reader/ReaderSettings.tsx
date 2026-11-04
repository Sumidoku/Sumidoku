import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import {
	AlignJustify,
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
import { Button, Select, Switch } from '../../components';
import {
	BLEND_MODES,
	DEFAULT_FILTERS,
	FILTER_LIMITS,
	hasActiveFilters,
	RGBA_ALPHA_LIMIT,
	RGBA_CHANNEL_LIMIT
} from './filters';
import type { BlendMode, FilterLevel, FilterLimit, ReaderFilters } from './filters';
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
	onModeChange: (mode: Mode) => void;
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
	textOnly: boolean;
	typography: ReaderTypography;
	onTypographyChange: (patch: Partial<ReaderTypography>) => void;
	filters: ReaderFilters;
	onFiltersChange: (filters: ReaderFilters) => void;
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

// One numeric filter (brightness etc.): a switch, with its slider indented
// underneath while enabled — Suwayomi's checkbox+slider pair
function FilterSlider({
	label,
	limit,
	level,
	format,
	onChange
}: {
	label: string;
	limit: FilterLimit;
	level: FilterLevel;
	format: (value: number) => string;
	onChange: (level: FilterLevel) => void;
}) {
	return (
		<div className="reader__filter">
			<SwitchRow label={label} checked={level.enabled} onChange={(enabled) => onChange({ ...level, enabled })} />
			{level.enabled && (
				<label className="reader__filter-slider">
					<input
						type="range"
						aria-label={label}
						min={limit.min}
						max={limit.max}
						step={limit.step}
						value={level.value}
						onChange={(e) => onChange({ ...level, value: Number(e.currentTarget.value) })}
					/>
					<output>{format(level.value)}</output>
				</label>
			)}
		</div>
	);
}

export function ReaderSettings({
	mode,
	onModeChange,
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
	textOnly,
	typography,
	onTypographyChange,
	filters,
	onFiltersChange
}: ReaderSettingsProps) {
	const { t } = useTranslation();
	const showTypography = pageText(currentPage) !== null;
	const pagedOnly = mode !== 'paged';
	const splitAvailable = layout === 'single';
	const turningNote = pagedOnly ? t('reader.pagedOnly') : undefined;
	return (
		<div className="reader__settings card">
			<SettingsGroup title={t('reader.readingMode')}>
				<OptionTiles
					label={t('reader.readingMode')}
					options={[
						{ value: 'paged', label: t('reader.mode.paged'), icon: <BookOpen size={18} /> },
						{ value: 'continuous', label: t('reader.mode.continuous'), icon: <AlignJustify size={18} /> },
						{ value: 'webtoon', label: t('reader.mode.webtoon'), icon: <StretchVertical size={18} /> }
					]}
					value={mode}
					onChange={onModeChange}
				/>
			</SettingsGroup>
			{!textOnly && (
				<SettingsGroup title={t('reader.groupPage')} note={pagedOnly ? t('reader.pagedOnly') : undefined}>
					<OptionTiles
						label={t('reader.layout')}
						options={[
							{ value: 'single', label: t('reader.layoutSingle'), icon: <Square size={18} /> },
							{ value: 'double', label: t('reader.layoutDouble'), icon: <BookOpen size={18} /> },
							{ value: 'doubleNoCover', label: t('reader.layoutDoubleNoCover'), icon: <BookOpen size={18} /> }
						]}
						value={layout}
						onChange={onLayoutChange}
					/>
					<OptionTiles
						label={t('reader.pageScale')}
						options={[
							{ value: 'screen', label: t('reader.scaleScreen'), icon: <Scan size={18} /> },
							{ value: 'width', label: t('reader.scaleWidth'), icon: <StretchHorizontal size={18} /> },
							{ value: 'widthShrink', label: t('reader.scaleWidthShrink'), icon: <Minimize2 size={18} /> },
							{ value: 'height', label: t('reader.scaleHeight'), icon: <StretchVertical size={18} /> },
							{ value: 'original', label: t('reader.scaleOriginal'), icon: <Frame size={18} /> }
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
			)}
			<SettingsGroup title={t('reader.groupTurning')} note={turningNote}>
				<OptionTiles
					label={t('reader.direction')}
					options={[
						{ value: 'ltr', label: t('reader.ltr'), icon: <ArrowRight size={18} /> },
						{ value: 'rtl', label: t('reader.rtl'), icon: <ArrowLeft size={18} /> }
					]}
					value={dir}
					onChange={onDirChange}
					columns={2}
				/>
				{!textOnly && (
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
								<span key={action}><svg className="reader__tz-swatch" viewBox="0 0 1 1" aria-hidden><rect width={1} height={1} className={`reader__tz-fill--${action}`} /></svg>{t(`reader.tzAction.${action}`)}</span>
							))}
						</div>
					</div>
				)}
				<div className="reader__settings-card">
					<SwitchRow label={t('reader.animations')} checked={anim} onChange={onAnimChange} />
					{!textOnly && anim && (
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
							{ value: 'width', label: t('reader.scaleWidth'), icon: <StretchHorizontal size={18} /> },
							{ value: 'original', label: t('reader.scaleOriginal'), icon: <FileImage size={18} /> }
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
			{!textOnly && (
				<SettingsGroup title={t('reader.groupFilters')}>
					<div className="reader__settings-card">
						<FilterSlider
							label={t('reader.filterBrightness')}
							limit={FILTER_LIMITS.brightness}
							level={filters.brightness}
							format={(v) => `${v}%`}
							onChange={(level) => onFiltersChange({ ...filters, brightness: level })}
						/>
						<FilterSlider
							label={t('reader.filterContrast')}
							limit={FILTER_LIMITS.contrast}
							level={filters.contrast}
							format={(v) => `${v}%`}
							onChange={(level) => onFiltersChange({ ...filters, contrast: level })}
						/>
						<FilterSlider
							label={t('reader.filterSaturate')}
							limit={FILTER_LIMITS.saturate}
							level={filters.saturate}
							format={(v) => `${v}%`}
							onChange={(level) => onFiltersChange({ ...filters, saturate: level })}
						/>
						<FilterSlider
							label={t('reader.filterHue')}
							limit={FILTER_LIMITS.hue}
							level={filters.hue}
							format={(v) => `${v}°`}
							onChange={(level) => onFiltersChange({ ...filters, hue: level })}
						/>
						<SwitchRow
							label={t('reader.filterSepia')}
							checked={filters.sepia}
							onChange={(sepia) => onFiltersChange({ ...filters, sepia })}
						/>
						<SwitchRow
							label={t('reader.filterGrayscale')}
							checked={filters.grayscale}
							onChange={(grayscale) => onFiltersChange({ ...filters, grayscale })}
						/>
						<SwitchRow
							label={t('reader.filterInvert')}
							checked={filters.invert}
							onChange={(invert) => onFiltersChange({ ...filters, invert })}
						/>
						<div className="reader__filter">
							<SwitchRow
								label={t('reader.filterTint')}
								checked={filters.rgba.enabled}
								onChange={(enabled) => onFiltersChange({ ...filters, rgba: { ...filters.rgba, enabled } })}
							/>
							{filters.rgba.enabled && (
								<>
									{(['red', 'green', 'blue'] as const).map((channel) => (
										<label key={channel} className="reader__filter-slider">
											<input
												type="range"
												aria-label={t(`reader.filterChannel.${channel}`)}
												min={RGBA_CHANNEL_LIMIT.min}
												max={RGBA_CHANNEL_LIMIT.max}
												step={RGBA_CHANNEL_LIMIT.step}
												value={filters.rgba[channel]}
												onChange={(e) =>
													onFiltersChange({
														...filters,
														rgba: { ...filters.rgba, [channel]: Number(e.currentTarget.value) }
													})
												}
											/>
											<output>
												{t(`reader.filterChannel.${channel}`)} {filters.rgba[channel]}
											</output>
										</label>
									))}
									<label className="reader__filter-slider">
										<input
											type="range"
											aria-label={t('reader.filterChannel.alpha')}
											min={RGBA_ALPHA_LIMIT.min}
											max={RGBA_ALPHA_LIMIT.max}
											step={RGBA_ALPHA_LIMIT.step}
											value={filters.rgba.alpha}
											onChange={(e) =>
												onFiltersChange({
													...filters,
													rgba: { ...filters.rgba, alpha: Number(e.currentTarget.value) }
												})
											}
										/>
										<output>{filters.rgba.alpha}%</output>
									</label>
									<div className="reader__settings-row">
										<span>{t('reader.filterBlendMode')}</span>
										<Select
											aria-label={t('reader.filterBlendMode')}
											value={filters.rgba.blendMode}
											onChange={(value) =>
												onFiltersChange({ ...filters, rgba: { ...filters.rgba, blendMode: value as BlendMode } })
											}
										>
											{BLEND_MODES.map((blendMode) => (
												<option key={blendMode} value={blendMode}>
													{t(`reader.filterBlend.${blendMode}`)}
												</option>
											))}
										</Select>
									</div>
								</>
							)}
						</div>
						<div className="reader__settings-row">
							<span>{t('reader.filterReset')}</span>
							<Button size="sm" onClick={() => onFiltersChange(DEFAULT_FILTERS)} disabled={!hasActiveFilters(filters)}>
								{t('reader.filterResetAction')}
							</Button>
						</div>
					</div>
				</SettingsGroup>
			)}
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
