export interface ProgressBarProps {
	/** 0~1; omitted or null means the total is unknown and the indeterminate animation is used */
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
