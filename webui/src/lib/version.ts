interface ParsedVersion {
	segments: number[];
	prerelease: string | null;
}

/** 剥 v 前缀后拆 `-` 预发布标签；数字段含非数字时算不可解析（调用方按不提示更新处理） */
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

/** 预发布互比：数字标识按数值、其余按字典序；前缀相同短者在前（semver 第 11 条） */
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
 * a 相对 b 的次序：负 = a 旧，0 = 同号，正 = a 新。预发布版排在同号正式版之前；
 * 数字段不等长按 0 补齐（1.2 与 1.2.0 同号）。任一侧不可解析返回 null
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
