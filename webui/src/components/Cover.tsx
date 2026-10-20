import { useEffect, useState } from 'react';

export interface CoverProps {
	src?: string | null;
	title: string;
	alt?: string;
}

/** Uniform 2:3 cover: shimmer while loading, title placeholder on failure (container query switches to a large first character at narrow sizes) */
export function Cover({ src, title, alt }: CoverProps) {
	const [failed, setFailed] = useState(false);
	const [loaded, setLoaded] = useState(false);
	// Failure is tracked per src: when the src is swapped in place (e.g. changing a cover), the old image's failure must not lock out the new one
	useEffect(() => {
		setFailed(false);
		setLoaded(false);
	}, [src]);
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
					onLoad={() => setLoaded(true)}
					onError={() => setFailed(true)}
				/>
			) : (
				<div className="cover__placeholder">
					<span className="cover__ph-text">{title ? title.slice(0, 6) : '无封面'}</span>
					<span className="cover__ph-char" aria-hidden="true">
						{title ? title.slice(0, 1) : '无'}
					</span>
				</div>
			)}
		</div>
	);
}
