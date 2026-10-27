import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { Check, Trash2 } from 'lucide-react';
import type { TrackerProvider } from '../api/types';
import { TRACKER_PROVIDER_NAMES } from '../api/trackers';
import { Cover } from './Cover';
import { Spinner } from './Spinner';
import { TrackerLogo } from './trackers/TrackerLogo';

export interface CoverCardProps {
	src?: string | null;
	title: string;
	subtitle?: ReactNode;
	/** Show an unread badge at the cover's top-right corner when > 0 */
	unreadCount?: number;
	trackerProviders?: TrackerProvider[];
	/** Local manga whose directory vanished: a trash chip at the top-left corner */
	trashed?: boolean;
	/** Pick one: internal navigation uses `to`, custom behavior (e.g. register-then-navigate) uses onClick */
	to?: string;
	onClick?: () => void;
	/** Dimmed while an async navigation triggered by the click is in flight, preventing double clicks */
	busy?: boolean;
	/** Selection mode: the card toggles instead of navigating, with a check overlay in the badge's slot */
	selecting?: boolean;
	selected?: boolean;
	onToggle?: () => void;
}

export function CoverCard({
	src,
	title,
	subtitle,
	unreadCount,
	trackerProviders,
	trashed,
	to,
	onClick,
	busy,
	selecting,
	selected,
	onToggle
}: CoverCardProps) {
	const { t } = useTranslation();
	const providers = trackerProviders ?? [];
	const providerNames = providers.map((provider) => TRACKER_PROVIDER_NAMES[provider]);
	const boundLabel = providers.length > 0 ? t('trackers.coverBound', { providers: providerNames.join(', ') }) : undefined;
	const inner = (
		<>
			<div className="cover-card__cover">
				<Cover src={src} title={title} />
				{!selecting && unreadCount != null && unreadCount > 0 && (
					<span className="cover-card__badge">{unreadCount > 99 ? '99+' : unreadCount}</span>
				)}
				{!selecting && boundLabel && (
					<span className="cover-card__trackers" title={boundLabel} aria-label={boundLabel}>
						{providers.slice(0, 2).map((provider) => (
							<TrackerLogo key={provider} provider={provider} />
						))}
						{providers.length > 2 && <span className="cover-card__trackers-more">+{providers.length - 2}</span>}
					</span>
				)}
				{trashed && (
					<span className="cover-card__trashed" title={t('common.trashed')}>
						<Trash2 size={12} />
					</span>
				)}
				{selecting && (
					<span className="cover-card__check">{selected && <Check size={13} strokeWidth={3} />}</span>
				)}
				{busy && (
					<div className="cover-card__busy">
						<Spinner />
					</div>
				)}
			</div>
			<div className="cover-card__title">{title}</div>
			{subtitle != null && <div className="cover-card__subtitle">{subtitle}</div>}
		</>
	);
	const cls = [
		'cover-card',
		selecting ? 'cover-card--selecting' : '',
		selecting && selected ? 'cover-card--selected' : '',
		busy ? 'cover-card--busy' : ''
	]
		.filter(Boolean)
		.join(' ');
	if (selecting) {
		return (
			<button type="button" className={cls} aria-pressed={selected ?? false} onClick={onToggle} disabled={busy}>
				{inner}
			</button>
		);
	}
	if (to) {
		return (
			<Link to={to} className={cls}>
				{inner}
			</Link>
		);
	}
	return (
		<button type="button" className={cls} onClick={onClick} disabled={busy}>
			{inner}
		</button>
	);
}
