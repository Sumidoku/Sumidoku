export interface SkippedItemsProps {
	label: string;
	items: string[];
	description?: string;
}

export function SkippedItems({ label, items, description }: SkippedItemsProps) {
	if (items.length === 0) return null;
	return (
		<details className="settings-backup__skipped">
			<summary>{label}</summary>
			{description && <p className="settings-modal-text">{description}</p>}
			<ul className="settings-backup__warnings">
				{items.map((item, i) => (
					<li key={i}>{item}</li>
				))}
			</ul>
		</details>
	);
}
