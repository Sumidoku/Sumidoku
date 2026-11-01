import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { RotateCcw, Search, SlidersHorizontal, X } from 'lucide-react';
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
	{ value: 'tracked', labelKey: 'library.filterTracked' },
	{ value: 'bookmarked', labelKey: 'library.filterBookmarked' },
	{ value: 'downloaded', labelKey: 'library.filterDownloaded' }
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
	authorTri: TriStateSelection,
	tagTri: TriStateSelection,
	groupId: number | null
): number {
	return (
		Object.keys(filterTri).length +
		Object.keys(ratings).length +
		Object.keys(sourceTri).length +
		Object.keys(authorTri).length +
		Object.keys(tagTri).length +
		(groupId === null ? 0 : 1)
	);
}

export type RemovableFilterKind = 'filter' | 'rating' | 'source' | 'author' | 'tag';

export interface ActiveFilterItem {
	id: string;
	label: string;
	state: 1 | -1;
	/** null is the selected filter group, which has no tri-state key to drop */
	kind: RemovableFilterKind | null;
	filterKey: string;
}

/** Everything advancedFilterCount counts, as summary pills. Search and categories stay out: their own controls are always on screen */
export function activeFilterItems(
	t: TFunction,
	selections: {
		filterTri: TriStateSelection;
		ratings: TriStateSelection;
		sourceTri: TriStateSelection;
		authorTri: TriStateSelection;
		tagTri: TriStateSelection;
		groupId: number | null;
		groups: FilterGroup[];
		sourceOptions: { key: string; label: string; name?: string }[];
	}
): ActiveFilterItem[] {
	const { filterTri, ratings, sourceTri, authorTri, tagTri, groupId, groups, sourceOptions } = selections;
	const named = (kind: string, value: string | undefined) =>
		value ? t('library.activeFilter', { kind, value }) : kind;
	const items: ActiveFilterItem[] = [];
	const add = (kind: RemovableFilterKind, states: TriStateSelection, label: (key: string) => string) => {
		for (const [filterKey, state] of Object.entries(states)) {
			items.push({ id: `${kind}:${filterKey}`, label: label(filterKey), state, kind, filterKey });
		}
	};
	add('filter', filterTri, (key) => {
		const option = FILTER_OPTIONS.find((o) => o.value === key);
		return option ? t(option.labelKey) : key;
	});
	add('rating', ratings, (key) =>
		named(t('library.rating'), MANGA_CONTENT_RATING_LEVELS.find((l) => String(l.value) === key)?.label)
	);
	// The source list may still be loading on a restored selection; a bare dimension label beats a raw id
	add('source', sourceTri, (key) => named(t('library.source'), sourceOptions.find((o) => o.key === key)?.name));
	add('author', authorTri, (key) => named(t('library.author'), key));
	add('tag', tagTri, (key) => named(t('library.tag'), key));
	if (groupId !== null) {
		items.push({
			id: 'group',
			label: named(t('library.group'), groups.find((g) => g.id === groupId)?.name),
			state: 1,
			kind: null,
			filterKey: ''
		});
	}
	return items;
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
	authorTri: TriStateSelection;
	tagTri: TriStateSelection;
	groupId: number | null;
	groups: FilterGroup[];
	categories: Category[];
	sourceOptions: { key: string; label: string; name?: string }[];
	authorOptions: { key: string; label: string }[];
	tagOptions: { key: string; label: string }[];
	showUncategorized: boolean;
	onFilterCycle: (key: string) => void;
	onRatingCycle: (key: string) => void;
	onSourceCycle: (key: string) => void;
	onCategoryCycle: (key: string) => void;
	onAuthorCycle: (key: string) => void;
	onTagCycle: (key: string) => void;
	onGroupChange: (id: number | null) => void;
	onRemoveFilter: (kind: RemovableFilterKind, key: string) => void;
	onResetRatings: () => void;
	onResetSources: () => void;
	onResetAuthors: () => void;
	onResetTags: () => void;
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
	authorTri,
	tagTri,
	groupId,
	groups,
	categories,
	sourceOptions,
	authorOptions,
	tagOptions,
	showUncategorized,
	onFilterCycle,
	onRatingCycle,
	onSourceCycle,
	onCategoryCycle,
	onAuthorCycle,
	onTagCycle,
	onGroupChange,
	onRemoveFilter,
	onResetRatings,
	onResetSources,
	onResetAuthors,
	onResetTags,
	onClearFilters
}: LibraryToolbarProps) {
	const { t } = useTranslation();
	const activeCount = advancedFilterCount(filterTri, ratings, sourceTri, authorTri, tagTri, groupId);
	const [filtersOpen, setFiltersOpen] = useState(false);
	const activeItems = activeFilterItems(t, { filterTri, ratings, sourceTri, authorTri, tagTri, groupId, groups, sourceOptions });
	// With a summary row on screen its clear button is the one; the panel keeps one for search and category selections
	const canClearInPanel = activeCount === 0 && (searchInput.trim() !== '' || Object.keys(categoryTri).length > 0);

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
						onChange={(value) => onSort(value as LibrarySort)}
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

			{activeItems.length > 0 && (
				<div className="library__active">
					<div className="library__active-filters" role="group" aria-label={t('library.activeFilters')}>
						{activeItems.map((item) => (
							<Chip
								key={item.id}
								triState={item.state}
								title={t(item.state === 1 ? 'library.activeIncluded' : 'library.activeExcluded', { label: item.label })}
								onClick={() => (item.kind === null ? onGroupChange(null) : onRemoveFilter(item.kind, item.filterKey))}
							>
								<span className="library__active-label">{item.label}</span>
								<X size={12} />
							</Chip>
						))}
					</div>
					<Button
						size="sm"
						className="library__active-clear"
						aria-label={t('library.clearFilters')}
						title={t('library.clearFilters')}
						onClick={onClearFilters}
					>
						<RotateCcw size={14} />
						<span>{t('library.clearFilters')}</span>
					</Button>
				</div>
			)}

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
							{/* With nothing selected and no names in the library the dropdown would open an empty menu; hide it.
							 * A restored selection still renders until the lists load and the page prunes stale names. */}
							{(authorOptions.length > 0 || Object.keys(authorTri).length > 0) && (
								<TriStateDropdown
									label={t('library.author')}
									options={authorOptions}
									states={authorTri}
									onCycle={onAuthorCycle}
									onReset={onResetAuthors}
								/>
							)}
							{(tagOptions.length > 0 || Object.keys(tagTri).length > 0) && (
								<TriStateDropdown
									label={t('library.tag')}
									options={tagOptions}
									states={tagTri}
									onCycle={onTagCycle}
									onReset={onResetTags}
								/>
							)}
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

					{canClearInPanel && (
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
