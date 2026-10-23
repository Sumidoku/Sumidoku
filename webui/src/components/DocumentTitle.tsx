import { createContext, useContext, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Outlet, useLocation, useMatches } from 'react-router-dom';

type TitleOverride = { pathname: string; title: string | null };
type TitleHandle = { title?: string };

const TitleOverrideContext = createContext<(override: TitleOverride) => void>(() => {});

export function DocumentTitleProvider() {
	const matches = useMatches();
	const { pathname } = useLocation();
	const { t } = useTranslation();
	const [override, setOverride] = useState<TitleOverride | null>(null);
	const handle = matches.at(-1)?.handle as TitleHandle | undefined;
	const fallback = handle?.title ? t(handle.title) : null;
	const overrideTitle = override?.pathname === pathname ? override.title : null;
	const title = overrideTitle ?? fallback;

	useEffect(() => {
		document.title = title ? `${title} · Sumidoku` : 'Sumidoku';
	}, [title]);

	return <TitleOverrideContext.Provider value={setOverride}><Outlet /></TitleOverrideContext.Provider>;
}

export function useDocumentTitle(title: string | null | undefined) {
	const setOverride = useContext(TitleOverrideContext);
	const { pathname } = useLocation();

	// Path scoping keeps a previous page's title from winning after navigation
	useEffect(() => {
		setOverride({ pathname, title: title || null });
	}, [pathname, title, setOverride]);
}
