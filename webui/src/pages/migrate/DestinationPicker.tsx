import { useState } from 'react';
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
	/** 已选目标源 id，顺序即优先级 */
	selected: number[];
	onChange: (ids: number[]) => void;
	/** 源条目所在的源，目标组合要排除 */
	excludeId?: number;
}

/**
 * 迁移目标组合选择器：SELECTED 有序（拖拽换序、可移除），AVAILABLE 点击追加，
 * 「全选置顶源」按 pin 顺序批量补入。单条目与批量两个入口共用。
 */
export function DestinationPicker({ selected, onChange, excludeId }: DestinationPickerProps) {
	const sources = useSources();
	const [dragIndex, setDragIndex] = useState<number | null>(null);
	const [overIndex, setOverIndex] = useState<number | null>(null);

	const all = (sources.data ?? []).filter((s) => s.id !== excludeId);
	const byId = new Map(all.map((s) => [s.id, s]));
	const chosen = selected.map((id) => byId.get(id)).filter((s): s is Source => s !== undefined);
	const chosenIds = new Set(chosen.map((s) => s.id));
	// 可选源按名字字母序；置顶仍按 pinOrder 段在前
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
				<h4>已选（顺序即优先级）</h4>
				{pinnedRest.length > 0 && (
					<Button size="sm" onClick={addAllPinned} title="按置顶顺序补入全部置顶源">
						<ListPlus size={14} />
						全选置顶源（{pinnedRest.length}）
					</Button>
				)}
			</div>
			{chosen.length === 0 ? (
				<p className="muted">还没选目标源，从下面点选；顺序决定自动匹配的优先级。</p>
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
								title={`移出 ${s.name}`}
								onClick={() => onChange(selected.filter((id) => id !== s.id))}
							>
								<X size={15} />
							</button>
						</div>
					))}
				</div>
			)}

			<h4>可选</h4>
			{available.length === 0 ? (
				<p className="muted">没有其他可加的源了。</p>
			) : (
				<div className="dest-picker__list">
					{available.map((s) => (
						<button
							key={s.id}
							type="button"
							className="dest-picker__row dest-picker__row--add"
							title={`加入 ${s.name}`}
							onClick={() => onChange([...selected, s.id])}
						>
							<RowIcon source={s} />
							<span className="dest-picker__name">{s.name}</span>
							{s.pinned && <span className="dest-picker__pin">置顶</span>}
							<Plus size={15} />
						</button>
					))}
				</div>
			)}
		</div>
	);
}
