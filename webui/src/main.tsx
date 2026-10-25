import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClientProvider } from '@tanstack/react-query';
import App from './App';
import { queryClient } from './api/hooks';
import { Toaster } from './components';
import './i18n';
import './styles/tokens.css';
import './styles/app.css';
import './components/components.css';
import './layout/layout.css';
import './pages/pages.css';

if (import.meta.env.PROD || import.meta.env.MODE === 'pwa') {
	void import('./pwa/register').then(({ registerPwa }) => registerPwa());
}

createRoot(document.getElementById('root')!).render(
	<StrictMode>
		<QueryClientProvider client={queryClient}>
			<App />
			<Toaster />
		</QueryClientProvider>
	</StrictMode>
);
