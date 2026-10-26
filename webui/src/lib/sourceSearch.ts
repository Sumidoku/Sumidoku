import type { FilterValue, SourceConfig } from '../api/types';

export interface SourceSearchSubmission {
	query: string;
	filters: FilterValue[];
}

export function sourceSearchOptions(
	config: SourceConfig | null | undefined,
	submitted: SourceSearchSubmission,
	isSearch: boolean,
	sourceEnabled: boolean
) {
	const dropFilters = !!config?.hidesFiltersWhileSearching && submitted.query !== '';
	return {
		request: {
			query: submitted.query || undefined,
			filters: dropFilters ? [] : submitted.filters
		},
		// Restored submissions must wait until the config can decide whether stored filters are allowed.
		enabled: isSearch && sourceEnabled && config !== undefined
	};
}
