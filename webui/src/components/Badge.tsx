import type { ReactNode } from 'react';

/** 小型计数角标；覆盖在封面右上角用 CoverCard 的 badge，这里只做内联场景 */
export function Badge({ children }: { children: ReactNode }) {
	return <span className="badge">{children}</span>;
}
