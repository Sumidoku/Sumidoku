// Types matching the sumidoku-server REST contract.
// Aidoku struct fields are snake_case; Sumidoku's own fields are camelCase.

export interface Source {
	id: number;
	key: string;
	name: string;
	/** Source kind; local/opds are built-in sources (no wasm instance; filters/listings/home/deep-link all 404) */
	kind: 'aidoku' | 'local' | 'opds';
	/** Source package version; the server side is i64 */
	version: number;
	languages: string[];
	/** Aidoku content rating; >1 counts as NSFW */
	contentRating: number;
	baseUrl: string | null;
	enabled: boolean;
	installedAt: number;
	updatedAt: number;
	/** Update version from each repo's cache; null when there is no repo or no update */
	updateVersion: number | null;
	/** Deprecation flag in Aidoku's sense: a repo-installed source no longer appears in any repo index and will not receive updates */
	missingFromRepos: boolean;
	/** Whether the source exports get_home (stamped at registration); null = unknown for old rows, entry stays visible */
	hasHome: boolean | null;
	/** manifest config.breakingChangeVersion, 0 when undeclared; cross-version updates trigger key migration */
	breakingChangeVersion: number;
	/** Raw manifest config section (Aidoku SourceInfo.Configuration), null when undeclared */
	config: SourceConfig | null;
	/** In the pinned list (browse.pinned_sources) */
	pinned: boolean;
	/** Index in the pinned list, i.e. the default priority on migration; null when not pinned */
	pinOrder: number | null;
}

/** Aidoku SourceInfo.Configuration; the DTO passes through verbatim, only the consumed keys are listed here */
export interface SourceConfig {
	/** Hide filters while searching, and search requests no longer carry filter values (Aidoku Runner semantics) */
	hidesFiltersWhileSearching?: boolean;
	/** tag clickable: declaration wins; clicking sends a Select filter with id "genre" and the raw tag as value (Aidoku matchingGenreFilter) */
	supportsTagSearch?: boolean;
	/** artist clickable: declaration wins; when undeclared, fall back to inferring from the filter list (a text filter with id "artist" exists) */
	supportsArtistSearch?: boolean;
	/** author clickable: declaration wins; when undeclared, fall back to inferring from the filter list (a text filter with id "author" exists) */
	supportsAuthorSearch?: boolean;
	/** Concurrent request cap for this source; the server turns it into a per-source admission cap on calls */
	maximumParallelRequests?: number;
}

export interface SourceRepo {
	id: number;
	url: string;
	name: string | null;
	sourceCount: number;
	addedAt: number;
	fetchedAt: number | null;
	/** Normalized message of the last refresh failure; the UI marks the repo stale from it */
	fetchError: string | null;
}

export interface RepoSourceEntry {
	key: string;
	name: string;
	version: number;
	languages: string[];
	contentRating: number;
	iconUrl: string | null;
	baseUrl: string | null;
	repoId: number;
	repoName: string | null;
	/** Present when installed, for single-source updates */
	installedId: number | null;
	installedVersion: number | null;
	updateVersion: number | null;
}

export interface UpdateAllOutcome {
	updated: string[];
	failed: { key: string; reason: string }[];
	/** Crosses breaking but the source cannot self-migrate: keys of sources that must go through the (same-source) migration page */
	migrationRequired: string[];
	/** Rows skipped during in-place rekeying (mapping failed/key collision), grouped by source */
	migrationWarnings: { key: string; message: string }[];
}

/** Single-source update response: the source DTO plus the key-migration outcome of this breaking-crossing update */
export interface SourceUpdate extends Source {
	migrationRequired: boolean;
	migrationWarnings: string[];
}

export interface Manga {
	key: string;
	title: string;
	cover: string | null;
	authors: string[] | null;
	artists: string[] | null;
	description: string | null;
	url: string | null;
	tags: string[] | null;
	status: string;
	content_rating: string;
	viewer: string;
	update_strategy: string;
	next_update_time: number | null;
	/** Attached by search/list endpoints; source-side Manga embedded in home components lacks it */
	sourceId?: number;
	/** Present once in the library */
	id?: number;
}

export interface MangaDetail extends Manga {
	id: number;
	initialized: boolean;
	chapterCount: number;
	inLibrary: boolean;
	incognito: boolean;
	categoryIds: number[];
}

export interface LibraryEntry extends Manga {
	id: number;
	inLibrary: boolean;
	inLibraryAt: number | null;
	unreadCount: number;
}

export interface Chapter {
	key: string;
	title: string | null;
	chapter_number: number | null;
	volume_number: number | null;
	date_uploaded: number | string | null;
	scanlators: string[] | null;
	url: string | null;
	language: string | null;
	thumbnail: string | null;
	locked: boolean;
	id: number;
	pageCount: number | null;
	lastPageRead: number;
	isRead: boolean;
	isBookmarked: boolean;
	lastReadAt: number | null;
	/** Chapters embedded in history/recent carry no download state */
	isDownloaded?: boolean;
}

export type PageContent =
	| { Url: [string, string | null] }
	| { Text: string }
	| { Zip: [string, string] }
	| { Image: unknown };

export interface PageData {
	index: number;
	content: PageContent | null;
	thumbnail: string | null;
	hasDescription: boolean;
	description: string | null;
}

export interface MangaPage {
	entries: Manga[];
	hasNextPage: boolean;
}

export interface LibraryPage {
	entries: LibraryEntry[];
	hasNextPage: boolean;
}

/** GET /library/source-counts: entry count per source in the library (including entries of disabled sources) */
export interface LibrarySourceCount {
	sourceId: number;
	count: number;
}

export interface HistoryEntry {
	chapter: Chapter;
	manga: MangaDetail;
	lastReadAt: number;
	lastPageRead: number;
	isRead: boolean;
}

export interface HistoryPage {
	entries: HistoryEntry[];
	hasNextPage: boolean;
}

export interface Category {
	id: number;
	name: string;
	sortOrder: number;
	isDefault: boolean;
	mangaCount: number;
	includeInDownload: number;
}

// ---- Library list filtering (query of GET /library) ----

export type LibrarySort = 'added' | 'title' | 'lastRead' | 'unread';

export type LibraryFilter = 'unread' | 'downloaded' | 'bookmarked';

/** Tri-state multi-select: keyed by option id (string form), 1 include / -1 exclude, unselected keys are absent */
export type TriStateSelection = Record<string, 1 | -1>;

// ---- Filter groups (Aidoku-style persisted rule sets) ----

/** Value-less flag types carry value: null; valued types carry their id/rating as a number */
export type FilterGroupRuleType =
	| 'downloaded'
	| 'has_unread'
	| 'started'
	| 'completed'
	| 'source'
	| 'content_rating'
	| 'category';

export interface FilterGroupRule {
	type: FilterGroupRuleType;
	value: number | null;
	exclude: boolean;
}

export interface FilterGroup {
	id: number;
	name: string;
	sortOrder: number;
	rules: FilterGroupRule[];
}

// ---- Downloads and updates ----

export type DownloadStatus = 'queued' | 'downloading' | 'paused' | 'finished' | 'error';

export interface DownloadItem {
	chapterId: number;
	mangaId: number;
	mangaTitle: string;
	chapterTitle: string | null;
	status: DownloadStatus;
	format: string;
	path: string | null;
	error: string | null;
	retries: number;
	/** Only has a value while downloading; null in every other state */
	downloaded: number | null;
	total: number | null;
	createdAt: number;
	updatedAt: number;
}

export type DownloadEventKind = 'queued' | 'started' | 'progress' | 'paused' | 'finished' | 'error';

/** /downloads/ws frames: the connection opens with one snapshot frame, then pushes events one by one */
export type DownloadWsFrame =
	| { type: 'snapshot'; running: boolean; downloads: DownloadItem[] }
	| { type: DownloadEventKind; chapterId: number; downloaded: number; total: number }
	// queue on/off change: chapterId/downloaded/total are fixed at 0, only running is meaningful
	| { type: 'running'; chapterId: 0; downloaded: 0; total: 0; running: boolean };

export interface UpdateSummary {
	running: boolean;
	total: number;
	complete: number;
	failed: number;
	skipped: number;
	pending: number;
	lastRunAt: number | null;
}

export type UpdateJobStatus = 'pending' | 'running' | 'complete' | 'failed' | 'skipped';

export interface UpdateJob {
	status: UpdateJobStatus;
	mangaId: number;
	mangaTitle: string;
	sourceId: number;
	addedChapters: number;
}

/** /update/ws frames: the connection opens with one summary snapshot, then pushes job events */
export type UpdateWsFrame =
	| ({ type: 'summary' } & UpdateSummary)
	| ({ type: 'job' } & UpdateJob);

export interface UpdateRecentEntry {
	chapter: Chapter;
	manga: MangaDetail;
	/** Written fetched_at in the contract; Sumidoku's own-field convention is camelCase — tolerate both */
	fetchedAt?: number | null;
	fetched_at?: number | null;
}

export interface UpdateRecentPage {
	entries: UpdateRecentEntry[];
	hasNextPage: boolean;
}

// ---- Source browsing ----

export interface Listing {
	id: string;
	name: string;
	kind: string;
}

/** Result of POST /sources/{id}/deep-link: source-side keys are echoed back verbatim,
 * while mangaId/chapterId are the server's internal ids after registering/matching by (source, key) */
export interface DeepLinkResult {
	mangaKey: string | null;
	chapterKey: string | null;
	listing: Listing | null;
	mangaId: number | null;
	chapterId: number | null;
}

export interface DeepLinkResponse {
	result: DeepLinkResult | null;
}

// A Filter's wire shape is aidoku's custom flat structure: a type tag plus the variant fields laid flat
export interface Filter {
	id: string;
	title: string | null;
	hide_from_header?: boolean | null;
	type: 'text' | 'sort' | 'check' | 'select' | 'multi-select' | 'note' | 'range';
	placeholder?: string | null;
	can_ascend?: boolean;
	options?: string[];
	default?: unknown;
	name?: string | null;
	can_exclude?: boolean;
	is_genre?: boolean;
	uses_tag_style?: boolean;
	ids?: (string | null)[] | null;
	default_included?: string[] | null;
	default_excluded?: string[] | null;
	text?: string;
	min?: number | null;
	max?: number | null;
	decimal?: boolean;
}

export type FilterValue =
	| { Text: { id: string; value: string } }
	| { Sort: { id: string; index: number; ascending: boolean } }
	| { Check: { id: string; value: number } }
	| { Select: { id: string; value: string } }
	| { MultiSelect: { id: string; included: string[]; excluded: string[] } }
	| { Range: { id: string; from: number | null; to: number | null } };

// ---- Source home (GET /sources/{id}/home, aidoku HomeLayout passed through verbatim) ----

export interface HomeLayout {
	components: HomeComponent[];
}

export interface HomeComponent {
	title: string | null;
	subtitle: string | null;
	value: HomeComponentValue;
}

export type HomeComponentValue =
	| {
			ImageScroller: {
				links: HomeLink[];
				auto_scroll_interval: number | null;
				width: number | null;
				height: number | null;
			};
	  }
	| { BigScroller: { entries: Manga[]; auto_scroll_interval: number | null } }
	| { Scroller: { entries: HomeLink[]; listing: Listing | null } }
	| {
			MangaList: {
				ranking: boolean;
				page_size: number | null;
				entries: HomeLink[];
				listing: Listing | null;
			};
	  }
	| {
			MangaChapterList: {
				page_size: number | null;
				entries: MangaWithChapter[];
				listing: Listing | null;
			};
	  }
	| { Filters: HomeFilterItem[] }
	| { Links: HomeLink[] };

export interface HomeLink {
	title: string;
	subtitle: string | null;
	image_url: string | null;
	value: HomeLinkValue | null;
}

export type HomeLinkValue = { Url: string } | { Listing: Listing } | { Manga: Manga };

export interface MangaWithChapter {
	manga: Manga;
	chapter: SourceChapter;
}

/** Source-side chapters embedded in home components/update streams (aidoku Chapter serialized verbatim), without server-attached fields */
export interface SourceChapter {
	key: string;
	title: string | null;
	chapter_number: number | null;
	volume_number: number | null;
	date_uploaded: number | string | null;
	scanlators: string[] | null;
	url: string | null;
	language: string | null;
	thumbnail: string | null;
	locked: boolean;
}

export interface HomeFilterItem {
	title: string;
	values: FilterValue[] | null;
}

/** Partial events of the streaming home: layout replaces everything; component replaces by index (index == current length means append) */
export type HomeStreamEvent =
	| { type: 'layout'; layout: HomeLayout }
	| { type: 'component'; index: number; component: HomeComponent };

/** Partial events of a streaming search: appended one by one as they arrive */
export type SearchStreamEvent = { type: 'manga'; manga: Manga };

/** One source's result in an aggregate search: a non-empty error means that source failed and entries is empty */
export interface SourceSearchResult {
	sourceId: number;
	entries: Manga[];
	hasNextPage: boolean;
	error?: { code: string; message: string };
}

export interface GlobalSearchResponse {
	sources: SourceSearchResult[];
}

/** Stream events of an aggregate search: one per source completion/failure. The failure type is deliberately not called error —
 * apiStream treats type:'error' as a terminal stream error and rejects the whole request, which would destroy per-source failure isolation */
export type GlobalSearchStreamEvent =
	| { type: 'source'; source: SourceSearchResult }
	| { type: 'sourceError'; sourceId: number; error: { code: string; message: string } };

/** Partial event of a streaming manga update: the complete source-side Manga snapshot at that moment (mirroring upstream's
 * partialMangaPublisher); details/chapters fill in as the fetch progresses, and the terminal event overwrites everything */
export type MangaUpdateStreamEvent = {
	type: 'manga';
	manga: Manga & { chapters?: SourceChapter[] };
};

// ---- Source settings ----

export type PageIcon =
	| { type: 'system'; name: string; color: string; inset?: number | null }
	| { type: 'url'; url: string };

export type SettingValue =
	| { Group: { footer?: string | null; items: Setting[] } }
	| {
			Page: {
				items: Setting[];
				inline_title?: boolean | null;
				auth_to_open?: boolean | null;
				icon?: PageIcon | null;
				info?: string | null;
			};
	  }
	| {
			Select: {
				values: string[];
				titles?: string[] | null;
				auth_to_open?: boolean | null;
				default?: string | null;
			};
	  }
	| {
			MultiSelect: {
				values: string[];
				titles?: string[] | null;
				auth_to_open?: boolean | null;
				default?: string[] | null;
			};
	  }
	| { Toggle: { subtitle?: string | null; auth_to_disable?: boolean | null; default: boolean } }
	| {
			Stepper: {
				minimum_value: number;
				maximum_value: number;
				step_value?: number | null;
				default?: number | null;
			};
	  }
	| { Segment: { options: string[]; default?: number | null } }
	| {
			Text: {
				placeholder?: string | null;
				secure?: boolean | null;
				default?: string | null;
			};
	  }
	// aidoku's unit variant serializes to a bare string without an object wrapper
	| 'Button'
	| { Button: Record<string, never> }
	| { Link: { url: string; external?: boolean | null } }
	| {
			Login: {
				method: 'basic' | 'oauth' | 'web';
				use_email?: boolean | null;
				url?: string | null;
				url_key?: string | null;
				logout_title?: string | null;
				local_storage_keys?: string[] | null;
				clear_cookies_on_log_out?: boolean | null;
				[key: string]: unknown;
			};
	  };

export interface Setting {
	key: string;
	title: string;
	type:
		| 'group'
		| 'select'
		| 'multi-select'
		| 'switch'
		| 'stepper'
		| 'segment'
		| 'text'
		| 'button'
		| 'link'
		| 'login'
		| 'page'
		| 'editable-list'
		| 'picker';
	notification?: string | null;
	requires?: string | null;
	requires_false?: string | null;
	refreshes?: string[] | null;
	value: SettingValue;
}

export interface SourceSettings {
	settings: Setting[] | null;
	values: Record<string, unknown>;
}

// ---- Endpoints added in this wave ----

export interface ScanOutcome {
	installed: string[];
	updated: string[];
	skipped: { dir: string; reason: string }[];
}

export type UpdateStrategy = 'Always' | 'Never';

export interface PatchMangaBody {
	updateStrategy?: UpdateStrategy;
	incognito?: boolean;
}

export interface PatchCategoryBody {
	name?: string;
	includeInDownload?: number;
}

export type ChapterPatchBody = {
	lastPageRead?: number;
	isRead?: boolean;
	isBookmarked?: boolean;
};

export interface ChapterProgress {
	chapterId: number;
	lastPageRead: number;
	isRead: boolean;
	isBookmarked: boolean;
	lastReadAt: number | null;
}

export interface BackupRestoreResult {
	restored: {
		library: number;
		categories: number;
		progress: number;
		settings: number;
		sourceSettings: number;
	};
	warnings: string[];
}

export interface BackupFileEntry {
	name: string;
	/** Byte count */
	size: number;
	/** UTC timestamp (seconds) embedded in the file name */
	createdAt: number;
}

export interface AidokuImportSkipped {
	section: 'library' | 'categories' | 'progress' | string;
	sourceKey: string;
	mangaKey: string;
	chapterKey?: string;
	reason: string;
}

export interface AidokuImportResult {
	dryRun: boolean;
	imported: {
		library: number;
		categories: number;
		progress: number;
	};
	skipped: AidokuImportSkipped[];
}

export interface SuwayomiImportSkipped {
	section: 'library' | 'progress' | string;
	/** Mihon source ids are int64 hashes that can exceed 2^53, so the server sends them as strings */
	mihonSourceId: string;
	mihonSourceName: string;
	mangaKey: string;
	reason: string;
}

export interface SuwayomiSourceGroup {
	mihonSourceId: string;
	mihonSourceName: string;
	mangaCount: number;
	/** auto = matched automatically by unique same name; mapped = explicitly mapped by the user; unmatched = no guessing, the whole group is skipped */
	match: 'auto' | 'mapped' | 'unmatched';
	aidokuSourceKey?: string;
	aidokuSourceName?: string;
	candidates: { key: string; name: string }[];
}

export interface SuwayomiImportResult {
	dryRun: boolean;
	imported: {
		library: number;
		categories: number;
		progress: number;
	};
	skipped: SuwayomiImportSkipped[];
	sources: SuwayomiSourceGroup[];
}

export interface BackupListResponse {
	/** The backup directory currently in effect on the server (env > setting > default) */
	dir: string;
	files: BackupFileEntry[];
}

// ---- Cross-source migration ----

export interface MigrateSource extends Source {
	/** Number of manga from this source in the library */
	mangaCount: number;
}

export interface MigrateLookupResult {
	/** The search term actually used (explicit query or the source entry's title) */
	query: string;
	candidates: Manga[];
}

export type MigrateMode = 'copy' | 'migrate' | 'migrateWithHistory';

export interface MigrateEntryBody {
	fromMangaId: number;
	toSourceId: number;
	toMangaKey: string;
	/** The candidate entry held by the client; the server passes it as the get_manga_update argument */
	toManga?: Manga;
}

export interface MigrateResultItem {
	fromMangaId: number;
	ok: boolean;
	toMangaId?: number;
	warnings?: string[];
	reason?: string;
	message?: string | null;
}

export interface MigrateResponse {
	results: MigrateResultItem[];
}

export interface MigrateRequest {
	mode: MigrateMode;
	entries: MigrateEntryBody[];
}

// ---- Users and auth ----

export interface AccountUser {
	id: number;
	username: string;
	role: 'admin' | 'reader';
}

/** GET /auth/status: when enabled is false, user is always null (an auth-less deployment treats everything as the built-in admin) */
export interface AuthStatus {
	enabled: boolean;
	user: AccountUser | null;
}

/** User-management list item; credential material never leaves the server, so there is no password field here */
export interface ManagedUser {
	id: number;
	username: string;
	role: 'admin' | 'reader';
	createdAt: number;
}
