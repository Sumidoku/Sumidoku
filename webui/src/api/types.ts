// 与 sumidoku-server REST 契约对应的类型。
// Aidoku 结构体字段为 snake_case，Sumidoku 自有字段为 camelCase。

export interface Source {
	id: number;
	key: string;
	name: string;
	/** 源包版本号，服务端是 i64 */
	version: number;
	languages: string[];
	/** Aidoku content rating，>1 视为 NSFW */
	contentRating: number;
	baseUrl: string | null;
	enabled: boolean;
	installedAt: number;
	updatedAt: number;
	/** 各源仓库缓存里的更新版本，无仓库或无更新为 null */
	updateVersion: number | null;
	/** manifest config.breakingChangeVersion，未声明为 0；跨版本更新会触发 key 迁移 */
	breakingChangeVersion: number;
	/** 在置顶列表（browse.pinned_sources）里 */
	pinned: boolean;
	/** 置顶列表里的下标，即迁移时的默认优先级；未置顶为 null */
	pinOrder: number | null;
}

export interface SourceRepo {
	id: number;
	url: string;
	name: string | null;
	sourceCount: number;
	addedAt: number;
	fetchedAt: number | null;
	/** 上次 refresh 失败的归一文案；界面据此标 stale */
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
	/** 已装则有内部 id，供单源更新用 */
	installedId: number | null;
	installedVersion: number | null;
	updateVersion: number | null;
}

export interface UpdateAllOutcome {
	updated: string[];
	failed: { key: string; reason: string }[];
	/** 跨 breaking 但源无自迁移能力：需要走迁移页（同源）的源 key */
	migrationRequired: string[];
	/** 原地重键时被跳过的行（映射失败/撞键），按源归组 */
	migrationWarnings: { key: string; message: string }[];
}

/** 单源更新响应：源 DTO 之外带本次跨 breaking 的 key 迁移结果 */
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
	/** 搜索/列表端点附加；主页组件内嵌的源侧 Manga 没有这个字段 */
	sourceId?: number;
	/** 已入库才有 */
	id?: number;
}

export interface MangaDetail extends Manga {
	id: number;
	initialized: boolean;
	chapterCount: number;
	inLibrary: boolean;
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
	/** history/recent 内嵌的章节不带下载态 */
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
	has_description: boolean;
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
	/** 三态：0 跟随全局 / 1 包含 / 2 排除 */
	includeInUpdate: number;
	includeInDownload: number;
}

// ---- 书库列表筛选（GET /library 的 query）----

export type LibrarySort = 'added' | 'title' | 'lastRead' | 'unread';

export type LibraryFilter = 'unread' | 'downloaded' | 'bookmarked';

// ---- 下载与更新 ----

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
	/** 只有下载中才有值，其余状态为 null */
	downloaded: number | null;
	total: number | null;
	createdAt: number;
	updatedAt: number;
}

export type DownloadEventKind = 'queued' | 'started' | 'progress' | 'paused' | 'finished' | 'error';

/** /downloads/ws 的帧：连接先给一帧 snapshot，之后逐条推事件 */
export type DownloadWsFrame =
	| { type: 'snapshot'; running: boolean; downloads: DownloadItem[] }
	| { type: DownloadEventKind; chapterId: number; downloaded: number; total: number }
	// 队列开关变化：chapterId/downloaded/total 固定为 0，只有 running 有效
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

/** /update/ws 的帧：连接先给一帧 summary 快照，之后推 job 事件 */
export type UpdateWsFrame =
	| ({ type: 'summary' } & UpdateSummary)
	| ({ type: 'job' } & UpdateJob);

export interface UpdateRecentEntry {
	chapter: Chapter;
	manga: MangaDetail;
	/** 契约写作 fetched_at，Sumidoku 自有字段惯例是 camelCase，两种都容忍 */
	fetchedAt?: number | null;
	fetched_at?: number | null;
}

export interface UpdateRecentPage {
	entries: UpdateRecentEntry[];
	hasNextPage: boolean;
}

// ---- 源浏览 ----

export interface Listing {
	id: string;
	name: string;
	kind: string;
}

// Filter 的 wire 形态是 aidoku 定制扁平结构：type 标签 + 变体字段平铺
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

// ---- 源主页（GET /sources/{id}/home，aidoku HomeLayout 原样透传）----

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

/** 主页组件内嵌的源侧章节，不带服务端附加字段 */
export interface SourceChapter {
	key: string;
	title: string | null;
	chapter_number: number | null;
	date_uploaded: number | string | null;
}

export interface HomeFilterItem {
	title: string;
	values: FilterValue[] | null;
}

/** 流式主页的部分事件：layout 全量替换；component 按下标替换（index == 当前长度即追加） */
export type HomeStreamEvent =
	| { type: 'layout'; layout: HomeLayout }
	| { type: 'component'; index: number; component: HomeComponent };

/** 流式搜索的部分事件：到达一条追加一条 */
export type SearchStreamEvent = { type: 'manga'; manga: Manga };

// ---- 源设置 ----

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
	// aidoku 的 unit variant 序列化成裸字符串，不带对象外壳
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
	/** 服务端能力面：browserLogin 为真时 web 登录项可走 sidecar 交互登录 */
	capabilities?: {
		browserLogin?: boolean;
	};
}

/** sidecar 交互登录的会话句柄 */
export interface BrowserLoginSession {
	session: string;
	loginUrl: string;
	expiresAt: number;
}

// ---- 本波新端点 ----

export interface ScanOutcome {
	installed: string[];
	updated: string[];
	skipped: { dir: string; reason: string }[];
}

export type UpdateStrategy = 'Always' | 'Never';

export interface PatchMangaBody {
	updateStrategy?: UpdateStrategy;
}

export interface PatchCategoryBody {
	name?: string;
	/** 三态：0 跟随全局 / 1 包含 / 2 排除 */
	includeInUpdate?: number;
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
	/** 字节数 */
	size: number;
	/** 文件名里的 UTC 时间戳（秒） */
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
	/** Mihon 源 id 是 int64 哈希可超 2^53，服务端按字符串下发 */
	mihonSourceId: string;
	mihonSourceName: string;
	mangaKey: string;
	reason: string;
}

export interface SuwayomiSourceGroup {
	mihonSourceId: string;
	mihonSourceName: string;
	mangaCount: number;
	/** auto=唯一同名自动匹配；mapped=用户显式映射；unmatched=不猜，整组跳过 */
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
	/** 服务端生效中的备份目录（env > setting > 默认） */
	dir: string;
	files: BackupFileEntry[];
}

// ---- 跨源迁移 ----

export interface MigrateSource extends Source {
	/** 该源在库的漫画数 */
	mangaCount: number;
}

export interface MigrateLookupResult {
	/** 实际生效的搜索词（显式 query 或源条目标题） */
	query: string;
	candidates: Manga[];
}

export type MigrateMode = 'copy' | 'migrate' | 'migrateWithHistory';

export interface MigrateEntryBody {
	fromMangaId: number;
	toSourceId: number;
	toMangaKey: string;
	/** 客户端手里的候选条目，服务端以它为 get_manga_update 入参 */
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
