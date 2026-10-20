import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { Spinner } from './Spinner';

export interface InfiniteListProps {
	hasNextPage: boolean;
	isFetching: boolean;
	onLoadMore: () => void;
	children: ReactNode;
	endText?: string;
}

/** IntersectionObserver sentinel + footer state; re-triggering is guarded internally by hasNextPage/isFetching */
export function InfiniteList({
	hasNextPage,
	isFetching,
	onLoadMore,
	children,
	endText = '没有更多了'
}: InfiniteListProps) {
	const sentinelRef = useRef<HTMLDivElement>(null);
	const stateRef = useRef({ hasNextPage, isFetching, onLoadMore });
	stateRef.current = { hasNextPage, isFetching, onLoadMore };

	useEffect(() => {
		const el = sentinelRef.current;
		if (!el) return;
		const ob = new IntersectionObserver((es) => {
			const s = stateRef.current;
			if (es[0].isIntersecting && s.hasNextPage && !s.isFetching) s.onLoadMore();
		});
		ob.observe(el);
		return () => ob.disconnect();
	}, []);

	return (
		<>
			{children}
			<div ref={sentinelRef} className="infinite-list__sentinel" />
			<div className="infinite-list__footer">
				{isFetching ? (
					<Spinner />
				) : !hasNextPage ? (
					<span className="infinite-list__end">{endText}</span>
				) : null}
			</div>
		</>
	);
}
