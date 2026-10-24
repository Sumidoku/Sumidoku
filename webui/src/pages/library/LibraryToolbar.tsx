import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { RotateCcw, Search, SlidersHorizontal } from 'lucide-react';
import type { Category, FilterGroup, LibraryFilter, LibrarySort } from '../../api/types';
import type { TriStateSelection } from '../../lib/triState';
import { MANGA_CONTENT_RATING_LEVELS } from '../../lib/source';
import { Button, Chip, Input, Select, TriStateDropdown } from '../../components';

export const UNCATEGORIZED = '0';

const SORT_OPTIONS: { value: LibrarySort; labelKey: string }[] = [
	{ value: 'added', labelKey: 'library.sortAdded' },
	{ value: 'title', labelKey: 'library.sortTitle' },
	{ value: 'lastRead', labelKey: 'library.sortLastRead' },
	{ value: 'latestChapter', labelKey: 'library.sortLatestChapter' },
	{ value: 'unread', labelKey: 'library.sortUnread' }
];

const FILTER_OPTIONS: { value: LibraryFilter; labelKey: string }[] = [
	{ value: 'unread', labelKey: 'library.filterUnread' },
	{ value: 'downloaded', labelKey: 'library.filterDownloaded' },
	{ value: 'bookmarked', labelKey: 'library.filterBookmarked' }
];

function triTitle(t: TFunction, label: string, state: 1 | -1 | 0): string {
	if (state === 1) return t('library.triInclude', { label });
	if (state === -1) return t('library.triExclude', { label });
	return t('library.triNone', { label });
}

export function advancedFilterCount(
	filterTri: TriStateSelection,
	ratings: TriStateSelection,
	sourceTri: TriStateSelection,
	groupId: number | null
): number {
	return Object.keys(filterTri).length + Object.keys(ratings).length + Object.keys(sourceTri).length + (groupId === null ? 0 : 1);
}

export interface LibraryToolbarProps {
	searchInput: string;
	onSearchInput: (value: string) => void;
	sort: LibrarySort;
	onSort: (value: LibrarySort) => void;
	filterTri: TriStateSelection;
	ratings: TriStateSelection;
	sourceTri: TriStateSelection;
	categoryTri: TriStateSelection;
	groupId: number | null;
	groups: FilterGroup[];
	categories: Category[];
	sourceOptions: { key: string; label: string }[];
	showUncategorized: boolean;
	onFilterCycle: (key: string) => void;
	onRatingCycle: (key: string) => void;
	onSourceCycle: (key: string) => void;
	onCategoryCycle: (key: string) => void;
	onGroupChange: (id: number | null) => void;
	onResetRatings: () => void;
	onResetSources: () => void;
	onClearFilters: () => void;
}

export function LibraryToolbar({
	searchInput,
	onSearchInput,
	sort,
	onSort,
	filterTri,
	ratings,
	sourceTri,
	categoryTri,
	groupId,
	groups,
	categories,
	sourceOptions,
	showUncategorized,
	onFilterCycle,
	onRatingCycle,
	onSourceCycle,
	onCategoryCycle,
	onGroupChange,
	onResetRatings,
	onResetSources,
	onClearFilters
}: LibraryToolbarProps) {
	const { t } = useTranslation();
	const activeCount = advancedFilterCount(filterTri, ratings, sourceTri, groupId);
	const [filtersOpen, setFiltersOpen] = useState(activeCount > 0);
	const canClear = searchInput.trim() !== '' || activeCount > 0 || Object.keys(categoryTri).length > 0;

	return (
		<section className="library__toolbar" aria-label={t('library.toolbar')}>
			<div className="library__primary">
				<Input
					className="library__search"
					icon={<Search size={16} />}
					type="search"
					placeholder={t('library.searchPlaceholder')}
					value={searchInput}
					onChange={(e) => onSearchInput(e.currentTarget.value)}
				/>
				<div className="library__actions">
					<Button
						className={filtersOpen || activeCount > 0 ? 'library__filter-toggle library__filter-toggle--active' : 'library__filter-toggle'}
						aria-expanded={filtersOpen}
						aria-controls="library-filter-panel"
						onClick={() => setFiltersOpen((open) => !open)}
					>
						<SlidersHorizontal size={16} />
						<span>{t('library.filters')}</span>
						{activeCount > 0 && <span className="library__filter-count">{activeCount}</span>}
					</Button>
					<Select
						className="library__sort"
						aria-label={t('library.sort')}
						value={sort}
						onChange={(e) => onSort(e.currentTarget.value as LibrarySort)}
					>
						{SORT_OPTIONS.map((option) => (
							<option key={option.value} value={option.value}>
								{t(option.labelKey)}
							</option>
						))}
					</Select>
				</div>
			</div>

			<div className="library__categories" role="group" aria-label={t('library.categories')}>
				{showUncategorized && (
					<Chip
						triState={categoryTri[UNCATEGORIZED] ?? 0}
						title={triTitle(t, t('library.uncategorized'), categoryTri[UNCATEGORIZED] ?? 0)}
						onClick={() => onCategoryCycle(UNCATEGORIZED)}
					>
						{t('library.uncategorized')}
					</Chip>
				)}
				{categories.map((category) => {
					const state = categoryTri[String(category.id)] ?? 0;
					return (
						<Chip
							key={category.id}
							triState={state}
							title={triTitle(t, category.name, state)}
							onClick={() => onCategoryCycle(String(category.id))}
						>
							{category.name}（{category.mangaCount}）
						</Chip>
					);
				})}
			</div>

			{filtersOpen && (
				<div className="library__filter-panel" id="library-filter-panel">
					<section className="library__filter-section">
						<h2 className="library__filter-title">{t('library.status')}</h2>
						<div className="library__filter-chips">
							{FILTER_OPTIONS.map((option) => {
								const state = filterTri[option.value] ?? 0;
								return (
									<Chip
										key={option.value}
										triState={state}
										title={triTitle(t, t(option.labelKey), state)}
										onClick={() => onFilterCycle(option.value)}
									>
										{t(option.labelKey)}
									</Chip>
								);
							})}
						</div>
					</section>

					<section className="library__filter-section">
						<h2 className="library__filter-title">{t('library.moreFilters')}</h2>
						<div className="library__filter-selects">
							<TriStateDropdown
								label={t('library.rating')}
								options={MANGA_CONTENT_RATING_LEVELS.map((level) => ({ key: String(level.value), label: level.label }))}
								states={ratings}
								onCycle={onRatingCycle}
								onReset={onResetRatings}
							/>
							<TriStateDropdown
								label={t('library.source')}
								options={sourceOptions}
								states={sourceTri}
								onCycle={onSourceCycle}
								onReset={onResetSources}
							/>
						</div>
					</section>

					{groups.length > 0 && (
						<section className="library__filter-section">
							<h2 className="library__filter-title">{t('library.filterGroups')}</h2>
							<div className="library__filter-chips">
								{groups.map((group) => (
									<Chip
										key={group.id}
										color={groupId === group.id ? 'accent' : 'default'}
										title={t('library.groupHint', { name: group.name })}
										onClick={() => onGroupChange(groupId === group.id ? null : group.id)}
									>
										{group.name}
									</Chip>
								))}
							</div>
						</section>
					)}

					{canClear && (
						<div className="library__filter-footer">
							<Button size="sm" onClick={onClearFilters}>
								<RotateCcw size={14} />
								{t('library.clearFilters')}
							</Button>
						</div>
					)}
				</div>
			)}
		</section>
	);
}
