export interface ProgressBarProps {
	/** 0~1；不传或 null 表示总量未知，走不定态动画 */
	value?: number | null;
}

export function ProgressBar({ value }: ProgressBarProps) {
	const pct = value != null && Number.isFinite(value) ? Math.min(100, Math.max(0, value * 100)) : null;
	return (
		<div className="progress" role="progressbar" aria-valuenow={pct ?? undefined}>
			{pct !== null ? (
				<div className="progress__fill" style={{ width: `${pct}%` }} />
			) : (
				<div className="progress__fill progress__fill--indeterminate" />
			)}
		</div>
	);
}
