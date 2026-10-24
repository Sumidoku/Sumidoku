import type { AuthStatus } from '../api/types';

export function authAllowsEvents(status: AuthStatus | undefined): boolean {
	return status !== undefined && (!status.enabled || status.user !== null);
}
