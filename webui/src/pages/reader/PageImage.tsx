import { useState } from 'react';
import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { ImageOff } from 'lucide-react';
import { Button } from '../../components';
import { imageErrored, imageRetried, imageStateFor, retrySrc } from './pageRetry';
import type { PageImageState } from './pageRetry';

export interface PageImageProps {
	src: string;
	alt: string;
	className?: string;
	style?: CSSProperties;
	width?: number;
	height?: number;
	lazy?: boolean;
	onLoad?: (width: number, height: number) => void;
}

/** A reader page image: the first load error is retried automatically once; if
    that also fails the page's slot keeps its box and shows a retry button */
export function PageImage({ src, alt, className, style, width, height, lazy, onLoad }: PageImageProps) {
	const { t } = useTranslation();
	const [state, setState] = useState<PageImageState | null>(null);
	const { attempt, failed } = imageStateFor(state, src);
	if (failed) {
		return (
			<div
				className={className ? `${className} reader__img-failed` : 'reader__img-failed'}
				style={style}
				role="alert"
			>
				<ImageOff size={20} aria-hidden />
				<span>{t('reader.pageFailed')}</span>
				<Button variant="primary" onClick={() => setState((s) => (s !== null ? imageRetried(s) : s))}>
					{t('reader.retry')}
				</Button>
			</div>
		);
	}
	return (
		<img
			className={className}
			src={retrySrc(src, attempt)}
			alt={alt}
			style={style}
			width={width}
			height={height}
			loading={lazy ? 'lazy' : undefined}
			draggable={false}
			onLoad={(e) => onLoad?.(e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)}
			onError={() => setState((s) => imageErrored(s, src))}
		/>
	);
}
