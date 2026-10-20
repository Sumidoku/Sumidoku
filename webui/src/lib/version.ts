interface ParsedVersion {
	segments: number[];
	prerelease: string | null;
}

/** Strip the v prefix, then split off the `-` prerelease tag; non-numeric segments count as unparseable (callers then skip the update prompt) */
function parseVersion(v: string): ParsedVersion | null {
	const core = v.trim().replace(/^v/i, '');
	const dash = core.indexOf('-');
	const numbers = (dash >= 0 ? core.slice(0, dash) : core).split('.');
	if (numbers.some((s) => !/^\d+$/.test(s))) return null;
	return {
		segments: numbers.map(Number),
		prerelease: dash >= 0 ? core.slice(dash + 1) : null
	};
}

/** Prerelease comparison: numeric identifiers numerically, the rest lexicographically; with a common prefix the shorter one comes first (semver rule 11) */
function comparePrerelease(a: string, b: string): number {
	const pa = a.split('.');
	const pb = b.split('.');
	for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
		if (i >= pa.length) return -1;
		if (i >= pb.length) return 1;
		const na = /^\d+$/.test(pa[i]) ? Number(pa[i]) : null;
		const nb = /^\d+$/.test(pb[i]) ? Number(pb[i]) : null;
		if (na !== null && nb !== null) {
			if (na !== nb) return na - nb;
		} else if (pa[i] !== pb[i]) {
			return pa[i] < pb[i] ? -1 : 1;
		}
	}
	return 0;
}

/**
 * Order of a relative to b: negative = a is older, 0 = same version, positive = a is newer. Prereleases
 * sort before the same-numbered release; unequal segment counts pad with 0 (1.2 equals 1.2.0). Either side unparseable returns null
 */
export function compareVersions(a: string, b: string): number | null {
	const pa = parseVersion(a);
	const pb = parseVersion(b);
	if (!pa || !pb) return null;
	for (let i = 0; i < Math.max(pa.segments.length, pb.segments.length); i++) {
		const diff = (pa.segments[i] ?? 0) - (pb.segments[i] ?? 0);
		if (diff !== 0) return diff;
	}
	if (pa.prerelease === pb.prerelease) return 0;
	if (pa.prerelease === null) return 1;
	if (pb.prerelease === null) return -1;
	return comparePrerelease(pa.prerelease, pb.prerelease);
}
