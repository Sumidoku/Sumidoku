import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { CircleQuestionMark } from 'lucide-react';
import { Button, EmptyState } from '../components';

export default function NotFound() {
	const { t } = useTranslation();
	return (
		<div className="page container">
			<EmptyState
				icon={CircleQuestionMark}
				title={t('notFound.title')}
				action={
					<Link to="/library">
						<Button variant="primary">{t('notFound.backToLibrary')}</Button>
					</Link>
				}
			/>
		</div>
	);
}
