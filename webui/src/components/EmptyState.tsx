import type { ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';

export interface EmptyStateProps {
	icon?: LucideIcon;
	title: string;
	hint?: string;
	action?: ReactNode;
}

export function EmptyState({ icon: Icon, title, hint, action }: EmptyStateProps) {
	return (
		<div className="empty-state">
			{Icon && (
				<div className="empty-state__icon">
					<Icon size={40} strokeWidth={1.5} />
				</div>
			)}
			<div className="empty-state__title">{title}</div>
			{hint && <div className="empty-state__hint">{hint}</div>}
			{action && <div className="empty-state__action">{action}</div>}
		</div>
	);
}
