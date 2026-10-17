// 与 suzuri-server REST 契约对应的类型。
// Aidoku 结构体字段为 snake_case，Suzuri 自有字段为 camelCase。

export interface Source {
	id: number;
	key: string;
	name: string;
	version: string;
	languages: string[];
	contentRating: string;
	baseUrl: string | null;
	enabled: boolean;
	installedAt: number;
	updatedAt: number;
}

export interface Manga {
	key: string;
	title: string;
	cover: string | null;
	authors: string[];
	artists: string[];
	description: string | null;
	url: string | null;
	tags: string[];
	status: string;
	content_rating: string;
	viewer: string;
	update_strategy: string;
	next_update_time: number | null;
	sourceId: number;
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
}

export interface Chapter {
	key: string;
	title: string | null;
	chapter_number: number | null;
	volume_number: number | null;
	date_uploaded: number | string | null;
	scanlators: string[];
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
	isDownloaded: boolean;
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
}

// ---- M3: 下载与更新 ----

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

export interface UpdateRecentEntry {
	chapter: Chapter;
	manga: MangaDetail;
	/** 契约写作 fetched_at，Suzuri 自有字段惯例是 camelCase，两种都容忍 */
	fetchedAt?: number | null;
	fetched_at?: number | null;
}

export interface UpdateRecentPage {
	entries: UpdateRecentEntry[];
	hasNextPage: boolean;
}

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

export type SettingValue =
	| { Group: { footer?: string | null; items: Setting[] } }
	| { Page: { items: Setting[]; inline_title?: boolean | null } }
	| {
			Select: {
				values: string[];
				titles?: string[] | null;
				default?: string | null;
			};
	  }
	| {
			MultiSelect: {
				values: string[];
				titles?: string[] | null;
				default?: string[] | null;
			};
	  }
	| { Toggle: { subtitle?: string | null; default: boolean } }
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
	// eslint-disable-next-line @typescript-eslint/no-empty-object-type
	| { Button: Record<string, never> }
	| { Link: { url: string; external?: boolean | null } }
	| { Login: Record<string, unknown> };

export interface Setting {
	key: string;
	title: string;
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
