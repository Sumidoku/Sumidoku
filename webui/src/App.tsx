import { createBrowserRouter, Navigate, RouterProvider } from 'react-router-dom';
import { AppLayout } from './layout/AppLayout';
import BrowsePage from './pages/BrowsePage';
import DownloadsPage from './pages/DownloadsPage';
import HistoryPage from './pages/HistoryPage';
import LibraryPage from './pages/LibraryPage';
import MangaDetailPage from './pages/MangaDetailPage';
import NotFound from './pages/NotFound';
import ReaderPage from './pages/ReaderPage';
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
			{ path: '/browse/:sourceId', element: <SourceBrowsePage /> },
			{ path: '/browse/:sourceId/home', element: <SourceHomePage /> },
			{ path: '/manga/:id', element: <MangaDetailPage /> },
			{ path: '/history', element: <HistoryPage /> },
			{ path: '/downloads', element: <DownloadsPage /> },
			{ path: '/updates', element: <UpdatesPage /> },
			{ path: '/settings', element: <SettingsPage /> },
			{ path: '*', element: <NotFound /> }
		]
	},
	// 阅读器全屏，不带 AppLayout 导航
	{ path: '/reader/:chapterId', element: <ReaderPage /> }
]);

export default function App() {
	return <RouterProvider router={router} />;
}
