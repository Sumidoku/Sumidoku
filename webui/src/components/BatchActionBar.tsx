import type { ReactNode } from 'react';

export interface BatchActionBarProps {
	/** Selected-count label; the page supplies the translated text */
	count: ReactNode;
	children: ReactNode;
	action?: ReactNode;
}

/** The bar floats over the list's last rows, so a selecting list pads its own container bottom */
export function BatchActionBar({ count, children, action }: BatchActionBarProps) {
	return (
		<div className="batch-bar">
			<span className="batch-bar__count">{count}</span>
			{children}
			{action && <span className="batch-bar__action">{action}</span>}
		</div>
	);
}
