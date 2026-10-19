import { useState } from 'react';
import { Check, Images, TriangleAlert } from 'lucide-react';
import { ApiError, sourceImageUrl } from '../../api/client';
import { useAlternateCovers, useSelectCover } from '../../api/hooks';
import type { MangaDetail } from '../../api/types';
import { Button, EmptyState, Modal, Spinner, pushToast } from '../../components';

interface CoverPickerModalProps {
	manga: MangaDetail;
	onClose: () => void;
}

export function CoverPickerModal({ manga, onClose }: CoverPickerModalProps) {
	const coversQuery = useAlternateCovers(manga.id);
	const selectCover = useSelectCover();
	const [pendingUrl, setPendingUrl] = useState<string | null>(null);

	function pick(url: string) {
		if (selectCover.isPending || url === manga.cover) return;
		setPendingUrl(url);
		selectCover.mutate(
			{ id: manga.id, url },
			{
				onSuccess: () => {
					pushToast('封面已更换', 'info');
					onClose();
				},
				onSettled: () => setPendingUrl(null)
			}
		);
	}

	// 源没导出 get_alternate_covers 时服务端按约定 501，按「没有备选」显示而不是报错
	const unsupported = coversQuery.error instanceof ApiError && coversQuery.error.status === 501;
	const covers = coversQuery.data ?? [];

	return (
		<Modal open title="更换封面" onClose={onClose} panelClassName="cover-picker">
			{coversQuery.isPending ? (
				<div className="cover-picker__loading">
					<Spinner size={20} />
				</div>
			) : unsupported ? (
				<EmptyState icon={Images} title="该源不提供备选封面" />
			) : coversQuery.isError ? (
				<EmptyState
					icon={TriangleAlert}
					title="备选封面加载失败"
					hint={coversQuery.error.message}
					action={
						<Button variant="primary" onClick={() => void coversQuery.refetch()}>
							重试
						</Button>
					}
				/>
			) : covers.length === 0 ? (
				<EmptyState icon={Images} title="源没有返回备选封面" />
			) : (
				<div className="cover-picker__grid">
					{covers.map((url) => {
						const current = url === manga.cover;
						return (
							<button
								key={url}
								type="button"
								className={
									current ? 'cover-picker__cell cover-picker__cell--current' : 'cover-picker__cell'
								}
								disabled={selectCover.isPending}
								title={current ? '当前封面' : '设为封面'}
								onClick={() => pick(url)}
							>
								{/* 防盗链源的图直连必 403，统一走服务端代理 */}
								<img src={sourceImageUrl(manga.sourceId!, url)} alt="" loading="lazy" />
								{current && (
									<span className="cover-picker__badge">
										<Check size={13} />
										当前
									</span>
								)}
								{pendingUrl === url && (
									<span className="cover-picker__busy">
										<Spinner size={16} />
									</span>
								)}
							</button>
						);
					})}
				</div>
			)}
		</Modal>
	);
}
