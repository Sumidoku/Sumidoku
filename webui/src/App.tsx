import { createBrowserRouter, Navigate, RouterProvider, useParams } from 'react-router-dom';
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
import SearchPage from './pages/SearchPage';
import SettingsPage from './pages/SettingsPage';
import SourceBrowsePage from './pages/SourceBrowsePage';
import SourceHomePage from './pages/SourceHomePage';
import UpdatesPage from './pages/UpdatesPage';

const router = createBrowserRouter([
	{
		element: <AppLayout />,
		children: [
			{ path: '/', element: <Navigate to="/library" replace /> },
			{ path: '/library', element: <LibraryPage /> },
			{ path: '/browse', element: <BrowsePage /> },
			{ path: '/browse/:sourceId', element: <SourceBrowseRoute /> },
			{ path: '/browse/:sourceId/home', element: <SourceHomePage /> },
			{ path: '/search', element: <SearchPage /> },
			{ path: '/manga/:id', element: <MangaDetailPage /> },
			{ path: '/migrate', element: <MigratePage /> },
			{ path: '/history', element: <HistoryPage /> },
			{ path: '/downloads', element: <DownloadsPage /> },
			{ path: '/updates', element: <UpdatesPage /> },
			{ path: '/settings', element: <SettingsPage /> },
			{ path: '*', element: <NotFound /> }
		]
	},
	// The reader is full-screen, without the AppLayout navigation
	{ path: '/reader/:chapterId', element: <ReaderPage /> },
	// The login page also has a standalone layout (no protected page renders while unauthenticated)
	{ path: '/login', element: <LoginPage /> }
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
