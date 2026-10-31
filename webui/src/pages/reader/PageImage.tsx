import { useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { ImageOff } from 'lucide-react';
import { Button } from '../../components';
import { imageErrored, imageLoading, imageRetried, imageStateFor, retrySrc } from './pageRetry';
import type { PageImageState } from './pageRetry';

export interface FailedPageProps {
	className?: string;
	style?: CSSProperties;
	/** Paged mode passes true: the in-slot button is unreachable under the tap
	    zones (the track's transform creates a stacking context no in-slot z-index
	    can escape), so the reader renders the retry as an overlay above the zones */
	retryInOverlay?: boolean;
	onRetry?: () => void;
}

/** A page whose load and automatic retry both failed: the slot keeps its box and
    shows the failure, with a manual retry button unless the parent overlays one */
export function FailedPage({ className, style, retryInOverlay, onRetry }: FailedPageProps) {
	const { t } = useTranslation();
	return (
		<div
			className={className ? `${className} reader__img-failed` : 'reader__img-failed'}
			style={style}
			role="alert"
		>
			<ImageOff size={20} aria-hidden />
			<span>{t('reader.pageFailed')}</span>
			{!retryInOverlay && (
				<Button
					variant="primary"
					onClick={(e) => {
						// Keep the tap from reaching the continuous container's click
						// handler, which would also toggle the toolbar
						e.stopPropagation();
						onRetry?.();
					}}
				>
					{t('reader.retry')}
				</Button>
			)}
		</div>
	);
}

export interface PageImageProps {
	src: string;
	alt: string;
	className?: string;
	style?: CSSProperties;
	width?: number;
	height?: number;
	lazy?: boolean;
	/** Paged mode passes true and renders the retry itself, above the tap zones */
	retryInOverlay?: boolean;
	/** Called once per latch when the automatic retry also fails */
	onFailed?: () => void;
	/** Bump to clear a latched failure and reload with a fresh cache-buster */
	retryToken?: number;
	onLoad?: (width: number, height: number) => void;
}

/** A reader page image: while the effective URL is in flight the img carries a
    `reader__img--loading` class that paints a spinner on its box; the first
    load error is retried automatically once, and if that also fails the page's
    slot keeps its box and shows a retry affordance */
export function PageImage({
	src,
	alt,
	className,
	style,
	width,
	height,
	lazy,
	retryInOverlay,
	onFailed,
	retryToken,
	onLoad
}: PageImageProps) {
	const [state, setState] = useState<PageImageState | null>(null);
	const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
	const { attempt, failed } = imageStateFor(state, src);
	useEffect(() => {
		if (failed) onFailed?.();
	}, [failed, onFailed]);
	useEffect(() => {
		// A token bump retries the latched failure the same way the in-slot button
		// does; a fresh mount (state still null) has nothing to unlatch
		if (retryToken) setState((s) => (s !== null ? imageRetried(s) : s));
	}, [retryToken]);
	if (failed) {
		return (
			<FailedPage
				className={className}
				style={style}
				retryInOverlay={retryInOverlay}
				onRetry={() => setState((s) => (s !== null ? imageRetried(s) : s))}
			/>
		);
	}
	const effectiveSrc = retrySrc(src, attempt);
	const cls = [className, imageLoading(loadedSrc, src, attempt) ? 'reader__img--loading' : null]
		.filter(Boolean)
		.join(' ');
	return (
		<img
			className={cls || undefined}
			src={effectiveSrc}
			alt={alt}
			style={style}
			width={width}
			height={height}
			loading={lazy ? 'lazy' : undefined}
			draggable={false}
			onLoad={(e) => {
				setLoadedSrc(effectiveSrc);
				onLoad?.(e.currentTarget.naturalWidth, e.currentTarget.naturalHeight);
			}}
			onError={() => setState((s) => imageErrored(s, src))}
		/>
	);
}
