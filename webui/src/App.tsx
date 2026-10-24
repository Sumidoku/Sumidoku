import { createBrowserRouter, Navigate, RouterProvider, useParams } from 'react-router-dom';
import { DocumentTitleProvider } from './components/DocumentTitle';
import { AppLayout } from './layout/AppLayout';
import BrowsePage from './pages/BrowsePage';
import DownloadsPage from './pages/DownloadsPage';
import HistoryPage from './pages/HistoryPage';
import LibraryPage from './pages/LibraryPage';
import LoginPage from './pages/LoginPage';
import MangaDetailPage from './pages/MangaDetailPage';
import MigratePage from './pages/migrate/MigratePage';
import NotFound from './pages/NotFound';
import ReaderPage from './pages/ReaderPage';
import SettingsPage from './pages/SettingsPage';
import SourceBrowsePage from './pages/SourceBrowsePage';
import SourceHomePage from './pages/SourceHomePage';
import UpdatesPage from './pages/UpdatesPage';

const router = createBrowserRouter([
	{
		element: <DocumentTitleProvider />,
		children: [
			{
				element: <AppLayout />,
				children: [
					{ path: '/', element: <Navigate to="/library" replace />, handle: { title: 'common.nav.library' } },
					{ path: '/library', element: <LibraryPage />, handle: { title: 'common.nav.library' } },
					{ path: '/browse', element: <BrowsePage />, handle: { title: 'common.nav.browse' } },
					{ path: '/browse/:sourceId', element: <SourceBrowseRoute />, handle: { title: 'sourceBrowse.sourceFallback' } },
					{ path: '/browse/:sourceId/home', element: <SourceHomePage />, handle: { title: 'sourceHome.sourceFallback' } },
					{ path: '/search', element: <Navigate to="/browse?view=search" replace />, handle: { title: 'common.nav.search' } },
					{ path: '/manga/:id', element: <MangaDetailPage />, handle: { title: 'mangaDetail.documentTitle' } },
					{ path: '/migrate', element: <MigratePage />, handle: { title: 'migrate.title' } },
					{ path: '/history', element: <HistoryPage />, handle: { title: 'common.nav.history' } },
					{ path: '/downloads', element: <DownloadsPage />, handle: { title: 'common.nav.downloads' } },
					{ path: '/updates', element: <UpdatesPage />, handle: { title: 'common.nav.updates' } },
					{ path: '/settings', element: <SettingsPage />, handle: { title: 'common.nav.settings' } },
					{ path: '*', element: <NotFound />, handle: { title: 'notFound.title' } }
				]
			},
			// The reader is full-screen, without the AppLayout navigation
			{ path: '/reader/:chapterId', element: <ReaderPage />, handle: { title: 'reader.documentTitleEmpty' } },
			// The login page also has a standalone layout (no protected page renders while unauthenticated)
			{ path: '/login', element: <LoginPage />, handle: { title: 'login.submit' } }
		]
	}
]);

// The browse page has state initialized once from the entry params (stored filter values, deep-link params);
// switching sourceId on the same route does not remount it, so force a reset with key to avoid filters leaking across sources
function SourceBrowseRoute() {
	const { sourceId } = useParams();
	return <SourceBrowsePage key={sourceId} />;
}

export default function App() {
	return <RouterProvider router={router} />;
}
