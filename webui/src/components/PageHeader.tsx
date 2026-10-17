import type { ReactNode } from 'react';

export function PageHeader({ title, actions }: { title: ReactNode; actions?: ReactNode }) {
	return (
		<header className="page-header">
			<h1 className="page-header__title">{title}</h1>
			{actions && <div className="page-header__actions">{actions}</div>}
		</header>
	);
}
