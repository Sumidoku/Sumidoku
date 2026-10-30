import anilist from '../../assets/trackers/anilist.png';
import bangumi from '../../assets/trackers/bangumi.svg';
import mal from '../../assets/trackers/mal.svg';
import mangabaka from '../../assets/trackers/mangabaka.png';
import type { TrackerProvider } from '../../api/types';
import { TRACKER_PROVIDER_NAMES } from '../../api/trackers';

const LOGOS: Record<TrackerProvider, string> = {
	anilist,
	bangumi,
	mal,
	mangabaka
};

export function TrackerLogo({ provider, className }: { provider: TrackerProvider; className?: string }) {
	return (
		<img
			className={[`tracker-logo tracker-logo--${provider}`, className].filter(Boolean).join(' ')}
			src={LOGOS[provider]}
			alt={TRACKER_PROVIDER_NAMES[provider]}
		/>
	);
}
