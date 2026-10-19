import { useEffect, useState } from 'react';

export interface CoverProps {
	src?: string | null;
	title: string;
	alt?: string;
}

/** 统一 2:3 封面：加载中显示 shimmer，失败回退标题占位（窄尺寸容器查询切成首字大字） */
export function Cover({ src, title, alt }: CoverProps) {
	const [failed, setFailed] = useState(false);
	const [loaded, setLoaded] = useState(false);
	// 失败是按 src 记的：换封面这类原地换 src 的场景，旧图的失败不能锁死新图
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
