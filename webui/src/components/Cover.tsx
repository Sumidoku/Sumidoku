import { useState } from 'react';
import { useTranslation } from 'react-i18next';

export interface CoverProps {
	src?: string | null;
	title: string;
	alt?: string;
}

/** Uniform 2:3 cover: shimmer while loading, title placeholder on failure (container query switches to a large first character at narrow sizes) */
export function Cover({ src, title, alt }: CoverProps) {
	const { t } = useTranslation();
	const [failedSrc, setFailedSrc] = useState<string | null>(null);
	const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
	const failed = src != null && failedSrc === src;
	const loaded = src != null && loadedSrc === src;
	const showImg = !!src && !failed;
	return (
		<div className={showImg && !loaded ? 'cover cover--shimmer' : 'cover'}>
			{showImg ? (
				<img
					className={loaded ? 'cover__img cover__img--loaded' : 'cover__img'}
					src={src}
					alt={alt || title}
					loading="lazy"
					referrerPolicy="no-referrer"
					ref={(image) => {
						// A cached image can already be complete before React delivers a load event
						if (!image?.complete) return;
						if (image.naturalWidth > 0) {
							setLoadedSrc(src ?? null);
						} else {
							setFailedSrc(src ?? null);
						}
					}}
					onLoad={() => setLoadedSrc(src ?? null)}
					onError={() => setFailedSrc(src ?? null)}
				/>
			) : (
				<div className="cover__placeholder">
					<span className="cover__ph-text">{title ? title.slice(0, 6) : t('cover.noCover')}</span>
					<span className="cover__ph-char" aria-hidden="true">
						{title ? title.slice(0, 1) : t('cover.noCoverChar')}
					</span>
				</div>
			)}
		</div>
	);
}
