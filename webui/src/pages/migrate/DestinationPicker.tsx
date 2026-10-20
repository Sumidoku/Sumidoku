import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { GripVertical, ListPlus, Plus, X } from 'lucide-react';
import { sourceIconUrl } from '../../api/client';
import { useSources } from '../../api/hooks';
import type { Source } from '../../api/types';
import { compareSourceName } from '../../lib/source';
import { Button } from '../../components';

function RowIcon({ source }: { source: Source }) {
	const [failed, setFailed] = useState(false);
	if (failed) {
		return <span className="dest-picker__icon dest-picker__icon--fallback">{source.name.slice(0, 1)}</span>;
	}
	return (
		<span className="dest-picker__icon">
			<img src={sourceIconUrl(source.id)} alt="" loading="lazy" onError={() => setFailed(true)} />
		</span>
	);
}

export interface DestinationPickerProps {
	/** Selected target source ids; the order is the priority */
	selected: number[];
	onChange: (ids: number[]) => void;
	/** The source of the entry being migrated; excluded from the destination set */
	excludeId?: number;
}

/**
 * Migration destination-set picker: SELECTED is ordered (drag to reorder, removable), AVAILABLE appends
 * on click, and "select all pinned" batch-adds in pin order. Shared by the single-entry and batch entries.
 */
export function DestinationPicker({ selected, onChange, excludeId }: DestinationPickerProps) {
	const { t } = useTranslation();
	const sources = useSources();
	const [dragIndex, setDragIndex] = useState<number | null>(null);
	const [overIndex, setOverIndex] = useState<number | null>(null);

	const all = (sources.data ?? []).filter((s) => s.id !== excludeId);
	const byId = new Map(all.map((s) => [s.id, s]));
	const chosen = selected.map((id) => byId.get(id)).filter((s): s is Source => s !== undefined);
	const chosenIds = new Set(chosen.map((s) => s.id));
	// Available sources sort alphabetically by name; pinned ones still lead by pinOrder
	const available = all
		.filter((s) => !chosenIds.has(s.id))
		.sort(compareSourceName);
	const pinnedRest = all
		.filter((s) => s.pinned && !chosenIds.has(s.id))
		.sort((a, b) => (a.pinOrder ?? 0) - (b.pinOrder ?? 0));

	function addAllPinned() {
		onChange([...selected, ...pinnedRest.map((s) => s.id)]);
	}

	function onDrop(target: number) {
		const from = dragIndex;
		setDragIndex(null);
		setOverIndex(null);
		if (from === null || from === target) return;
		const next = [...selected];
		const [moved] = next.splice(from, 1);
		next.splice(target, 0, moved);
		onChange(next);
	}

	return (
		<div className="dest-picker">
			<div className="dest-picker__head">
				<h4>{t('destinationPicker.selectedHeading')}</h4>
				{pinnedRest.length > 0 && (
					<Button size="sm" onClick={addAllPinned} title={t('destinationPicker.addAllPinnedTitle')}>
						<ListPlus size={14} />
						{t('destinationPicker.addAllPinned', { count: pinnedRest.length })}
					</Button>
				)}
			</div>
			{chosen.length === 0 ? (
				<p className="muted">{t('destinationPicker.emptyHint')}</p>
			) : (
				<div className="dest-picker__list">
					{chosen.map((s, i) => (
						<div
							key={s.id}
							className={[
								'dest-picker__row',
								dragIndex === i ? 'dest-picker__row--dragging' : '',
								overIndex === i && dragIndex !== null && dragIndex !== i
									? 'dest-picker__row--over'
									: ''
							]
								.filter(Boolean)
								.join(' ')}
							onDragOver={(e) => {
								e.preventDefault();
								setOverIndex(i);
							}}
							onDrop={() => onDrop(i)}
						>
							<span
								className="dest-picker__grip"
								draggable
								onDragStart={(e) => {
									e.dataTransfer.setData('text/plain', s.key);
									e.dataTransfer.effectAllowed = 'move';
									setDragIndex(i);
								}}
								onDragEnd={() => {
									setDragIndex(null);
									setOverIndex(null);
								}}
							>
								<GripVertical size={15} />
							</span>
							<span className="dest-picker__order">{i + 1}</span>
							<RowIcon source={s} />
							<span className="dest-picker__name">{s.name}</span>
							<button
								type="button"
								className="dest-picker__remove"
								title={t('destinationPicker.remove', { name: s.name })}
								onClick={() => onChange(selected.filter((id) => id !== s.id))}
							>
								<X size={15} />
							</button>
						</div>
					))}
				</div>
			)}

			<h4>{t('destinationPicker.availableHeading')}</h4>
			{available.length === 0 ? (
				<p className="muted">{t('destinationPicker.noMore')}</p>
			) : (
				<div className="dest-picker__list">
					{available.map((s) => (
						<button
							key={s.id}
							type="button"
							className="dest-picker__row dest-picker__row--add"
							title={t('destinationPicker.add', { name: s.name })}
							onClick={() => onChange([...selected, s.id])}
						>
							<RowIcon source={s} />
							<span className="dest-picker__name">{s.name}</span>
							{s.pinned && <span className="dest-picker__pin">{t('destinationPicker.pinned')}</span>}
							<Plus size={15} />
						</button>
					))}
				</div>
			)}
		</div>
	);
}
