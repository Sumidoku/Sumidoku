import type { ReactNode } from 'react';

export interface TabItem {
	key: string;
	label: ReactNode;
	/** 角标计数（如分类下漫画数），null/undefined 不显示 */
	badge?: number | null;
}

export interface TabsProps {
	tabs: TabItem[];
	active: string;
	onChange: (key: string) => void;
}

/** underline 款页签，横向可滚 */
export function Tabs({ tabs, active, onChange }: TabsProps) {
	return (
		<div className="tabs" role="tablist">
			{tabs.map((t) => (
				<button
					key={t.key}
					type="button"
					role="tab"
					aria-selected={t.key === active}
					className={t.key === active ? 'tabs__tab tabs__tab--active' : 'tabs__tab'}
					onClick={() => onChange(t.key)}
				>
					{t.label}
					{t.badge != null && <span className="tabs__badge">{t.badge}</span>}
				</button>
			))}
		</div>
	);
}
