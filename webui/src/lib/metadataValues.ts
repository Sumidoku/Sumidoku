/**
 * The apply payload for a match field the user edited. A list is typed comma-separated; a cleared
 * input means no value, so it must clear the field rather than fall back to the matched value.
 */
export function editedMetadataValue(incoming: unknown, text: string): string | string[] | null {
	if (Array.isArray(incoming)) {
		return text
			.split(',')
			.map((item) => item.trim())
			.filter((item) => item.length > 0);
	}
	return text.trim() === '' ? null : text;
}
