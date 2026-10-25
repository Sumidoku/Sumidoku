/** Download enqueue targets: local-source chapters are already on disk and a trashed entry's files are unavailable */
export function downloadEligibleIds(
	entries: { id: number; sourceId: number; trashed: boolean }[],
	localSourceIds: ReadonlySet<number>
): number[] {
	return entries.filter((e) => !e.trashed && !localSourceIds.has(e.sourceId)).map((e) => e.id);
}
