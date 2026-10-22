import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Check, Images, TriangleAlert } from 'lucide-react';
import { ApiError, sourceImageUrl } from '../../api/client';
import { useAlternateCovers, useSelectCover } from '../../api/hooks';
import type { Manga } from '../../api/types';
import { Button, EmptyState, Modal, Spinner, pushToast } from '../../components';

interface CoverPickerModalProps {
	manga: Manga;
	onClose: () => void;
}

export function CoverPickerModal({ manga, onClose }: CoverPickerModalProps) {
	const { t } = useTranslation();
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
					pushToast(t('coverPicker.changed'), 'info');
					onClose();
				},
				onSettled: () => setPendingUrl(null)
			}
		);
	}

	// When the source doesn't export get_alternate_covers the server 501s by convention; show as "no alternates" instead of an error
	const unsupported = coversQuery.error instanceof ApiError && coversQuery.error.status === 501;
	const covers = coversQuery.data ?? [];

	return (
		<Modal open title={t('coverPicker.title')} onClose={onClose} panelClassName="cover-picker">
			{coversQuery.isPending ? (
				<div className="cover-picker__loading">
					<Spinner size={20} />
				</div>
			) : unsupported ? (
				<EmptyState icon={Images} title={t('coverPicker.unsupported')} />
			) : coversQuery.isError ? (
				<EmptyState
					icon={TriangleAlert}
					title={t('coverPicker.loadFailed')}
					hint={coversQuery.error.message}
					action={
						<Button variant="primary" onClick={() => void coversQuery.refetch()}>
							{t('coverPicker.retry')}
						</Button>
					}
				/>
			) : covers.length === 0 ? (
				<EmptyState icon={Images} title={t('coverPicker.empty')} />
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
								title={current ? t('coverPicker.currentTitle') : t('coverPicker.setAsCover')}
								onClick={() => pick(url)}
							>
								{/* Images of anti-hotlink sources always 403 on direct hits, so they all go through the server proxy */}
								<img src={sourceImageUrl(manga.sourceId!, url)} alt="" loading="lazy" />
								{current && (
									<span className="cover-picker__badge">
										<Check size={13} />
										{t('coverPicker.currentBadge')}
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
