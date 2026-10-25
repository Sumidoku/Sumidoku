// GET /api/v1/events 的 SSE 帧类型。通用流式事件（home/search/manga-update）
// 的类型在 types.ts，这里只收下载/更新/snapshot 这几种实时推送帧，避免重复。
import type { DownloadEventKind, DownloadItem, UpdateJob, UpdateSummary } from './types';

/** event:download 帧——服务端只带 chapterId 与字节计数，整表元数据要另拉 */
export type DownloadChapterEvent = {
	type: DownloadEventKind;
	chapterId: number;
	downloaded: number;
	total: number;
};

/** event:snapshot 帧——连接首帧与每次 resync 都走它，带下载全表 + 更新摘要 */
export type EventSnapshot = {
	type: 'snapshot';
	running: boolean;
	downloads: DownloadItem[];
	updateSummary: UpdateSummary;
};

/** event:update 帧——单条库更新任务的状态流转（服务端 kind 固定为 "job"） */
export type UpdateEvent = { type: 'job' } & UpdateJob;
