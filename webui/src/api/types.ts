// Generated from crates/server/assets/api-docs/openapi.json; do not edit.

export type AddRepoRequest = {
	url: string;
};

export type AddToLibraryRequest = {
	mangaId: number;
	categoryIds?: Array<number> | null;
};

export type AidokuImportResponse = {
	dryRun: boolean;
	imported: ImportCounts;
	skipped: Array<AidokuSkipped>;
};

export type AidokuListing = {
	id: string;
	name: string;
	kind: "Default" | "List";
};

export type AidokuSkipped = {
	section: string;
	sourceKey: string;
	mangaKey: string;
	chapterKey?: string;
	reason: string;
};

export type ApiError = {
	code: "bad_request" | "unauthorized" | "not_found" | "unsupported" | "payload_too_large" | "source_disabled" | "duplicate" | "no_update" | "resync_required" | "not_in_library" | "bad_gateway" | "source_error" | "timeout" | "internal" | "busy" | "task_finished" | "not_trashed" | "gone" | "root_unavailable";
	message: string;
};

export type ApiKeyCreateRequest = {
	name: string;
};

export type ApiKeyCreated = ApiKeyEntry & {
	key: string;
};

export type ApiKeyEntry = {
	id: string;
	name: string;
	createdAt: number;
	lastUsedAt: number | null;
	source: "ui";
};

export type AuthStatus = {
	enabled: boolean;
	username?: string;
	user: AuthUser | null;
};

export type AuthUser = {
	id: number;
	username: string;
};

export type BackupCategory = {
	name: string;
	sortOrder?: number;
	isDefault?: boolean;
	includeInUpdate?: number;
	includeInDownload?: number;
	manga?: Array<BackupMangaRef>;
};

export type BackupChapter = {
	key: string;
	title?: string | null;
	number?: number | null;
	volume?: number | null;
	scanlators?: Array<string>;
	language?: string | null;
	thumbnail?: string | null;
	url?: string | null;
	dateUploaded?: number | null;
	sourceOrder?: number;
	pageCount?: number;
};

export type BackupDoc = {
	version: 1;
	createdAt?: number;
	sources?: Array<BackupSource>;
	library?: Array<BackupManga>;
	categories?: Array<BackupCategory>;
	progress?: Array<BackupProgress>;
	settings?: {
		[key: string]: JsonValue;
	};
	sourceSettings?: {
		[key: string]: SettingsValues;
	};
};

export type BackupFile = {
	name: string;
	size: number;
	createdAt: number;
};

export type BackupFiles = {
	dir: string;
	files: Array<BackupFile>;
};

export type BackupManga = {
	sourceKey: string;
	key: string;
	title?: string;
	authors?: Array<string>;
	artists?: Array<string>;
	description?: string | null;
	tags?: Array<string>;
	status?: number;
	contentRating?: number;
	viewer?: number;
	coverUrl?: string | null;
	url?: string | null;
	initialized?: boolean;
	updateStrategy?: number;
	inLibrary?: boolean;
	inLibraryAt?: number | null;
	viewerFlags?: number;
	incognito?: boolean;
	sortKey?: string;
};

export type BackupMangaRef = {
	sourceKey: string;
	mangaKey: string;
};

export type BackupProgress = {
	sourceKey: string;
	mangaKey: string;
	chapter: BackupChapter;
	lastPageRead?: number;
	isRead?: boolean;
	isBookmarked?: boolean;
	lastReadAt?: number | null;
};

export type BackupRestoreResponse = {
	restored: {
		library: number;
		categories: number;
		progress: number;
		settings: number;
		sourceSettings: number;
	};
	warnings: Array<string>;
};

export type BackupSettings = {
	enabled: boolean;
	intervalHours: number;
	retentionCount: number;
	lastRun: number | null;
	dir: string | null;
};

export type BackupSource = {
	key: string;
	name: string;
	version: number;
	repoUrl?: string | null;
};

export type BasicLoginRequest = {
	key: string;
	username: string;
	password: string;
};

export type BatchProgressRequest = ProgressPatch & {
	chapterIds: Array<number>;
};

export type BrowseSettings = {
	pinnedSources: Array<string>;
};

export type Category = {
	id: number;
	name: string;
	sortOrder: number;
	isDefault: boolean;
	mangaCount: number;
};

export type CategoryNameRequest = {
	name: string;
};

export type Chapter = SourceChapter & {
	id: number;
	trashed: boolean;
	pageCount: number;
	lastPageRead: number;
	isRead: boolean;
	isBookmarked: boolean;
	lastReadAt: number | null;
};

export type ChapterInput = {
	key: string;
	title?: string | null;
	chapter_number?: number | null;
	volume_number?: number | null;
	date_uploaded?: number | null;
	scanlators?: Array<string> | null;
	url?: string | null;
	language?: string | null;
	thumbnail?: string | null;
	locked: boolean;
};

export type ChapterProgress = {
	chapterId: number;
	lastPageRead: number;
	isRead: boolean;
	isBookmarked: boolean;
	lastReadAt: number | null;
};

export type ChapterSort = "source" | "chapter" | "date";

export type ChapterWithDownload = Chapter & {
	isDownloaded: boolean;
};

export type ClearedResponse = {
	cleared: number;
};

export type ContentRating = "Unknown" | "Safe" | "Suggestive" | "NSFW";

export type CreateFilterGroupRequest = {
	name: string;
	rules?: Array<FilterRuleInput>;
};

export type DeepLinkRequest = {
	url: string;
};

export type DeepLinkResponse = {
	result: DeepLinkTarget | null;
};

export type DeepLinkTarget = {
	mangaKey: string | null;
	chapterKey: string | null;
	listing: AidokuListing | null;
	mangaId: number | null;
	chapterId: number | null;
};

export type Download = {
	chapterId: number;
	mangaId: number;
	mangaTitle: string;
	chapterTitle: string | null;
	chapterNumber: number | null;
	volumeNumber: number | null;
	status: DownloadStatus;
	format: string;
	path: string | null;
	error: string | null;
	retries: number;
	downloaded: number | null;
	total: number | null;
	createdAt: number;
	updatedAt: number;
};

export type DownloadSettings = {
	autoDownloadNewChapters: boolean;
	autoDownloadLimit: number;
	downloadFormat: "cbz" | "folder";
	downloadConcurrency: number;
	deleteAfterManualRead: boolean;
	deleteAfterRead: boolean;
	allowDeleteBookmarked: boolean;
	autoSkipMangaWithUnread: boolean;
	autoIgnoreReuploads: boolean;
	downloadAhead: boolean;
	downloadAheadCount: number;
	movePath: string | null;
};

export type DownloadStatus = "queued" | "downloading" | "paused" | "finished" | "error";

export type EnqueueDownloadsRequest = {
	chapterIds: Array<number>;
};

export type Error = {
	error: ApiError;
};

export type FetchUpdatesRequest = {
	categoryId?: number | null;
};

export type FilterGroup = {
	id: number;
	name: string;
	sortOrder: number;
	rules: Array<FilterRule>;
};

export type FilterRule = {
	type: FilterRuleType;
	value: number | null;
	exclude: boolean;
};

export type FilterRuleInput = {
	type: FilterRuleType;
	value?: number | string | null;
	exclude?: boolean;
};

export type FilterRuleType = "downloaded" | "has_unread" | "started" | "completed" | "source" | "content_rating" | "category";

export type FilterValue = {
	Text: {
		id: string;
		value: string;
	};
} | {
	Sort: {
		id: string;
		index: number;
		ascending: boolean;
	};
} | {
	Check: {
		id: string;
		value: number;
	};
} | {
	Select: {
		id: string;
		value: string;
	};
} | {
	MultiSelect: {
		id: string;
		included: Array<string>;
		excluded: Array<string>;
	};
} | {
	Range: {
		id: string;
		from?: number | null;
		to?: number | null;
	};
};

export type GlobalSearchRequest = {
	query?: string | null;
	page?: number;
	sourceIds: Array<number>;
};

export type GlobalSearchResponse = {
	sources: Array<GlobalSearchResult>;
};

export type GlobalSearchResult = MangaPage & {
	sourceId: number;
	error?: {
		code: string;
		message: string;
	};
};

export type HistoryEntry = {
	chapter: Chapter;
	manga: Manga;
	lastReadAt: number;
	lastPageRead: number;
	isRead: boolean;
};

export type HistoryPage = {
	entries: Array<HistoryEntry>;
	hasNextPage: boolean;
};

export type HomeComponent = {
	title: string | null;
	subtitle: string | null;
	value: HomeComponentValue;
};

export type HomeComponentValue = {
	ImageScroller: {
		links: Array<Link>;
		auto_scroll_interval: number | null;
		width: number | null;
		height: number | null;
	};
} | {
	BigScroller: {
		entries: Array<SourceManga>;
		auto_scroll_interval: number | null;
	};
} | {
	Scroller: {
		entries: Array<Link>;
		listing: AidokuListing | null;
	};
} | {
	MangaList: {
		ranking: boolean;
		page_size: number | null;
		entries: Array<Link>;
		listing: AidokuListing | null;
	};
} | {
	MangaChapterList: {
		page_size: number | null;
		entries: Array<MangaWithChapter>;
		listing: AidokuListing | null;
	};
} | {
	Filters: Array<HomeFilterItem>;
} | {
	Links: Array<Link>;
};

export type HomeFilterItem = {
	title: string;
	values: Array<FilterValue> | null;
};

export type HomeLayout = {
	components: Array<HomeComponent>;
};

export type IdResponse = {
	id: number;
};

export type ImportCounts = {
	library: number;
	categories: number;
	progress: number;
};

export type IncognitoSettings = {
	enabled: boolean;
};

export type InstallSourceRequest = {
	repoId: number;
	sourceKey: string;
};

export type JsonValue = unknown;

export type LibraryEntry = Manga & {
	inLibraryAt: number | null;
	unreadCount: number;
};

export type LibraryFilter = "unread" | "downloaded" | "bookmarked";

export type LibraryPage = {
	entries: Array<LibraryEntry>;
	hasNextPage: boolean;
};

export type LibrarySettings = {
	defaultCategory: string;
	showUncategorized: boolean;
	excludedUpdateCategories: Array<number>;
	excludedDownloadCategories: Array<number>;
	skipTitles: Array<string>;
	refreshMetadata: boolean;
	updateIntervalHours: number;
};

export type LibrarySort = "added" | "title" | "lastRead" | "latestChapter" | "unread";

export type Link = {
	title: string;
	subtitle: string | null;
	image_url: string | null;
	value: LinkValue | null;
};

export type LinkValue = {
	Url: string;
} | {
	Listing: AidokuListing;
} | {
	Manga: SourceManga;
};

export type Listing = {
	id: string;
	name: string;
	kind: "default" | "list";
};

export type LocalTrashEntry = {
	manga: {
		id: number;
		sourceId: number;
		key: string;
		title: string;
	};
	trashedAt: number;
	trashedChapters: number;
};

export type LoginRequest = {
	username: string;
	password: string;
};

export type LoginResponse = {
	token: string;
	expiresAt: number;
	user: AuthUser;
};

export type Manga = SourceManga & {
	id: number;
	sourceId: number;
	initialized: boolean;
	trashed: boolean;
	chapterCount: number;
	inLibrary: boolean;
	incognito: boolean;
	categoryIds: Array<number>;
};

export type MangaInput = {
	key: string;
	title: string;
	cover?: string | null;
	artists?: Array<string> | null;
	authors?: Array<string> | null;
	description?: string | null;
	url?: string | null;
	tags?: Array<string> | null;
	status: MangaStatus;
	content_rating: ContentRating;
	viewer: Viewer;
	update_strategy: UpdateStrategy;
	next_update_time?: number | null;
	chapters?: Array<ChapterInput> | null;
};

export type MangaPage = {
	entries: Array<SearchManga>;
	hasNextPage: boolean;
	degraded?: boolean;
};

export type MangaStatus = "Unknown" | "Ongoing" | "Completed" | "Cancelled" | "Hiatus";

export type MangaWithChapter = {
	manga: SourceManga;
	chapter: SourceChapter;
};

export type MigrateEntry = {
	fromMangaId: number;
	toSourceId: number;
	toMangaKey: string;
	toManga?: MangaInput | null;
};

export type MigrateMode = "copy" | "migrate" | "migrateWithHistory";

export type MigrateRequest = {
	mode: MigrateMode;
	entries: Array<MigrateEntry>;
};

export type MigrateResponse = {
	results: Array<MigrateResult>;
};

export type MigrateResult = {
	fromMangaId: number;
	ok: boolean;
	toMangaId?: number;
	warnings?: Array<string>;
	reason?: "not_found" | "source_not_found" | "same_source" | "not_in_library" | "fetch_failed";
	message?: string | null;
};

export type MigrationLookupResponse = {
	query: string;
	candidates: Array<SearchManga>;
	degraded?: boolean;
};

export type MigrationSource = Source & {
	mangaCount: number;
};

export type NotifyRequest = {
	notification: string;
};

export type OAuthCompleteRequest = {
	key: string;
	url: string;
};

export type OAuthStartRequest = {
	key: string;
};

export type Ok = {
	ok: true;
};

export type Page = {
	content: PageContent;
	thumbnail: string | null;
	hasDescription: boolean;
	description: string | null;
	index: number;
	width?: number;
	height?: number;
};

export type PageContent = {
	Url: [string, {
		[key: string]: string;
	} | null];
} | {
	Text: string;
} | {
	Image: string;
} | {
	Zip: [string, string];
} | {
	Local: string;
} | {
	OpdsZip: string;
} | {
	Downloaded: string;
};

export type PageIcon = {
	type: "system";
	name: string;
	color: string;
	inset?: number | null;
} | {
	type: "url";
	url: string;
};

export type PasswordChangeRequest = {
	currentPassword: string;
	newPassword: string;
};

export type ProgressPatch = {
	lastPageRead?: number;
	isRead?: boolean;
	isBookmarked?: boolean;
};

export type QueuedResponse = {
	queued: Array<number>;
};

export type RecentEntry = {
	chapter: Chapter;
	manga: Manga;
	fetchedAt: number;
};

export type RecentPage = {
	entries: Array<RecentEntry>;
	hasNextPage: boolean;
};

export type RegisterInternalRequest = RegisterOpdsInstance | RegisterLocalInstance;

export type RegisterLocalInstance = {
	type: "local";
	name: string;
	paths: Array<string>;
	oneshotsDir?: string | null;
};

export type RegisterMangaRequest = {
	sourceId: number;
	mangaKey?: string;
	manga?: MangaInput | null;
};

export type RegisterOpdsInstance = {
	type: "opds";
	name: string;
	url: string;
	username?: string | null;
	password?: string | null;
};

export type ReorderPinnedRequest = {
	ids: Array<string>;
};

export type ReorderRequest = {
	ids: Array<number>;
};

export type RepoSource = {
	key: string;
	name: string;
	version: number;
	languages: Array<string>;
	contentRating: number;
	iconUrl: string | null;
	baseUrl: string | null;
	repoId: number;
	repoName: string | null;
	installedId: number | null;
	installedVersion: number | null;
	updateVersion: number | null;
};

export type RestoreBackupRequest = {
	name: string;
};

export type RunningResponse = {
	running: boolean;
};

export type ScanOutcome = {
	installed: Array<string>;
	updated: Array<string>;
	skipped: Array<SkippedSource>;
};

export type SearchManga = SourceManga & {
	sourceId: number;
	id?: number;
};

export type SearchRequest = {
	query?: string | null;
	page?: number;
	filters?: Array<FilterValue> | null;
};

export type SelectCoverRequest = {
	url: string;
};

export type ServerSettings = {
	incognito: IncognitoSettings;
	library: LibrarySettings;
	download: DownloadSettings;
	backup: BackupSettings;
	browse: BrowseSettings;
	[key: string]: JsonValue;
};

export type SetCategoriesRequest = {
	categoryIds: Array<number>;
};

export type SettingValue = string | {
	Group: {
		footer: string | null;
		items: Array<SourceSetting>;
	};
} | {
	Select: {
		values: Array<string>;
		titles: Array<string> | null;
		auth_to_open: boolean | null;
		default: string | null;
	};
} | {
	MultiSelect: {
		values: Array<string>;
		titles: Array<string> | null;
		auth_to_open: boolean | null;
		default: Array<string> | null;
	};
} | {
	Toggle: {
		subtitle: string | null;
		auth_to_disable: boolean | null;
		default: boolean;
	};
} | {
	Stepper: {
		minimum_value: number;
		maximum_value: number;
		step_value: number | null;
		default: number | null;
	};
} | {
	Segment: {
		options: Array<string>;
		default: number | null;
	};
} | {
	Text: {
		placeholder: string | null;
		autocapitalization_type: number | null;
		autocorrection_disabled: boolean | null;
		keyboard_type: number | null;
		return_key_type: number | null;
		secure: boolean | null;
		default: string | null;
	};
} | {
	Link: {
		url: string;
		external: boolean | null;
	};
} | {
	Login: {
		method: "basic" | "oauth" | "web";
		url: string | null;
		url_key: string | null;
		logout_title: string | null;
		pkce: boolean | null;
		token_url: string | null;
		callback_scheme: string | null;
		use_email: boolean | null;
		local_storage_keys: Array<string> | null;
		clear_cookies_on_log_out: boolean | null;
	};
} | {
	Page: {
		items: Array<SourceSetting>;
		inline_title: boolean | null;
		auth_to_open: boolean | null;
		icon: PageIcon | null;
		info: string | null;
	};
} | {
	EditableList: {
		line_limit: number | null;
		inline: boolean | null;
		placeholder: string | null;
		default: Array<string> | null;
	};
} | {
	Picker: {
		values: Array<string>;
		titles: Array<string> | null;
		default: string | null;
	};
};

export type Settings = {
	"incognito.enabled"?: boolean;
	"library.default_category"?: string;
	"library.show_uncategorized"?: boolean;
	"library.excluded_update_categories"?: Array<number>;
	"library.excluded_download_categories"?: Array<number>;
	"library.skip_titles"?: Array<"hasUnread" | "completed" | "notStarted">;
	"library.refresh_metadata"?: boolean;
	"library.update_interval_hours"?: number;
	auto_download_new_chapters?: boolean;
	auto_download_limit?: number;
	download_format?: "cbz" | "folder";
	download_concurrency?: number;
	delete_after_manual_read?: boolean;
	delete_after_read?: boolean;
	allow_delete_bookmarked?: boolean;
	"downloads.auto_skip_manga_with_unread"?: boolean;
	"downloads.auto_ignore_reuploads"?: boolean;
	"downloads.download_ahead"?: boolean;
	"downloads.download_ahead_count"?: number;
	"local.dirs"?: Array<string>;
	"local.oneshots_dir"?: string;
	[key: string]: JsonValue;
};

export type SettingsValues = {
	[key: string]: JsonValue;
};

export type SkippedSource = {
	dir: string;
	reason: string;
};

export type SortOrder = "asc" | "desc";

export type Source = {
	id: number;
	key: string;
	name: string;
	kind: "aidoku" | "local" | "opds";
	version: number;
	languages: Array<string>;
	contentRating: number;
	baseUrl: string | null;
	breakingChangeVersion: number;
	config: SourceConfig | null;
	enabled: boolean;
	installedAt: number;
	updatedAt: number;
	updateVersion: number | null;
	missingFromRepos: boolean;
	hasHome: boolean | null;
	pinned: boolean;
	pinOrder: number | null;
};

export type SourceChapter = {
	key: string;
	title: string | null;
	chapter_number: number | null;
	volume_number: number | null;
	date_uploaded: number | null;
	scanlators: Array<string> | null;
	url: string | null;
	language: string | null;
	thumbnail: string | null;
	locked: boolean;
};

export type SourceConfig = {
	languageSelectType?: "single" | "multiple";
	supportsArtistSearch?: boolean;
	supportsAuthorSearch?: boolean;
	supportsTagSearch?: boolean;
	allowsBaseUrlSelect?: boolean;
	breakingChangeVersion?: number;
	hidesFiltersWhileSearching?: boolean;
	maximumParallelRequests?: number;
	[key: string]: JsonValue;
};

export type SourceCount = {
	sourceId: number;
	count: number;
};

export type SourceFilter = {
	id: string;
	title: string | null;
	hide_from_header: boolean | null;
	type: string;
	placeholder?: string | null;
	can_ascend?: boolean | null;
	options?: Array<string> | null;
	default?: JsonValue;
	name?: string | null;
	can_exclude?: boolean | null;
	is_genre?: boolean | null;
	uses_tag_style?: boolean | null;
	ids?: Array<string> | null;
	default_included?: Array<string> | null;
	default_excluded?: Array<string> | null;
	text?: string | null;
	min?: number | null;
	max?: number | null;
	decimal?: boolean | null;
};

export type SourceLogoutRequest = {
	key: string;
	clearCookies?: boolean;
};

export type SourceManga = {
	key: string;
	title: string;
	cover: string | null;
	artists: Array<string> | null;
	authors: Array<string> | null;
	description: string | null;
	url: string | null;
	tags: Array<string> | null;
	status: MangaStatus;
	content_rating: ContentRating;
	viewer: Viewer;
	update_strategy: UpdateStrategy;
	next_update_time: number | null;
	chapters: Array<SourceChapter> | null;
};

export type SourcePatch = {
	pinned?: boolean;
	enabled?: boolean;
};

export type SourceRepo = {
	id: number;
	url: string;
	name: string | null;
	sourceCount: number;
	addedAt: number;
	fetchedAt: number | null;
	fetchError: string | null;
};

export type SourceSetting = {
	type: string;
	key: string;
	title: string | null;
	notification: string | null;
	requires: string | null;
	requires_false: string | null;
	refreshes: Array<string> | null;
	value: SettingValue;
};

export type SourceSettings = {
	settings: Array<SourceSetting>;
	values: SettingsValues;
};

export type SourceWithMigration = Source & {
	migrationRequired: boolean;
	migrationWarnings: Array<string>;
};

export type StartedResponse = {
	started: true;
};

export type SuwayomiImportResponse = {
	dryRun: boolean;
	imported: ImportCounts;
	skipped: Array<SuwayomiSkipped>;
	sources: Array<SuwayomiSourceReport>;
};

export type SuwayomiSkipped = {
	section: string;
	mihonSourceId: string;
	mihonSourceName: string;
	mangaKey: string;
	reason: string;
};

export type SuwayomiSourceReport = {
	mihonSourceId: string;
	mihonSourceName: string;
	mangaCount: number;
	match: "mapped" | "auto" | "unmatched";
	candidates: Array<{
		key: string;
		name: string;
	}>;
	aidokuSourceKey?: string;
	aidokuSourceName?: string;
	realignedKeys?: {
		manga?: number;
		chapter?: number;
	};
};

export type SyncApplyResult = {
	originSequence: number;
	status: "applied" | "duplicate" | "stale" | "skipped";
	reason?: "source_unavailable";
};

export type SyncChange = {
	origin: string;
	originUserId: 1;
	originSequence: number;
	entityKind: "manga" | "manga_user" | "category" | "category_manga" | "chapter" | "chapter_user";
	entityKey: string;
	deleted: boolean;
	modifiedAt: number;
	identity: SyncIdentity;
	snapshot: SyncSnapshot;
};

export type SyncCompactResponse = {
	baseCursor: number;
	removed: number;
};

export type SyncIdentity = {
	[key: string]: JsonValue;
};

export type SyncPullResponse = {
	origin: string;
	userId: number;
	nextCursor: number;
	hasMore: boolean;
	changes: Array<SyncChange>;
};

export type SyncPushRequest = {
	origin: string;
	changes: Array<SyncChange>;
};

export type SyncPushResponse = {
	results: Array<SyncApplyResult>;
};

export type SyncSnapshot = {
	lastModifiedAt: number;
	[key: string]: JsonValue;
};

export type Task = {
	id: string;
	kind: string;
	sourceId: number | null;
	payload: JsonValue;
	priority: number;
	status: "pending" | "running" | "done" | "failed" | "cancelled";
	createdAt: number;
	startedAt: number | null;
	finishedAt: number | null;
	error: string | null;
	result: JsonValue;
};

export type TextPage = {
	type: "text";
	text: string;
};

export type UpdateAllSourcesResponse = {
	updated: Array<string>;
	failed: Array<{
		key: string;
		reason: string;
	}>;
	migrationRequired: Array<string>;
	migrationWarnings: Array<{
		key: string;
		message: string;
	}>;
};

export type UpdateCategoryRequest = {
	name?: string;
};

export type UpdateFilterGroupRequest = {
	name?: string;
	rules?: Array<FilterRuleInput>;
};

export type UpdateMangaRequest = {
	updateStrategy?: UpdateStrategy;
	incognito?: boolean;
};

export type UpdateStrategy = "Always" | "Never";

export type UpdateSummary = {
	running: boolean;
	total: number;
	complete: number;
	failed: number;
	skipped: number;
	pending: number;
	lastRunAt: number | null;
};

export type UpdatedResponse = {
	updated: number;
};

export type UrlResponse = {
	url: string;
};

export type ValuesResponse = {
	values: SettingsValues;
};

export type Version = {
	version: string;
};

export type Viewer = "Unknown" | "LeftToRight" | "RightToLeft" | "Vertical" | "Webtoon";

export type WebLoginRequest = {
	key: string;
	cookies: {
		[key: string]: string;
	};
	localStorage?: {
		[key: string]: string;
	};
};
