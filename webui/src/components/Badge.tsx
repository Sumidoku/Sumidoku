import type { ReactNode } from 'react';

/** Small count badge; for overlaying on a cover's top-right corner use CoverCard's badge — this one is only for inline use */
export function Badge({ children }: { children: ReactNode }) {
	return <span className="badge">{children}</span>;
}
