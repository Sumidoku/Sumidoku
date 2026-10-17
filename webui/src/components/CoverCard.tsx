import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { Cover } from './Cover';
import { Spinner } from './Spinner';

export interface CoverCardProps {
	src?: string | null;
	title: string;
	subtitle?: ReactNode;
	/** >0 时在封面右上角显示未读角标 */
	unreadCount?: number;
	/** 二选一：内部跳转走 to，自定义行为（如先注册再跳）走 onClick */
	to?: string;
	onClick?: () => void;
	/** 点击后异步跳转中置灰防连点 */
	busy?: boolean;
}

export function CoverCard({ src, title, subtitle, unreadCount, to, onClick, busy }: CoverCardProps) {
	const inner = (
		<>
			<div className="cover-card__cover">
				<Cover src={src} title={title} />
				{unreadCount != null && unreadCount > 0 && (
					<span className="cover-card__badge">{unreadCount > 99 ? '99+' : unreadCount}</span>
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
	const cls = busy ? 'cover-card cover-card--busy' : 'cover-card';
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
