import { useTranslation } from 'react-i18next';

export function Spinner({ size }: { size?: number }) {
	const { t } = useTranslation();
	return (
		<span
			className="spinner"
			style={size ? { width: size, height: size } : undefined}
			role="status"
			aria-label={t('spinner.loading')}
		/>
	);
}
