// window.history.state?.idx counts SPA navigations: 0 (or a missing state) means the page was
// entered directly (fresh load/deep link), where navigate(-1) would leave the app entirely —
// callers fall back to a fixed in-app destination in that case.
export function hasHistoryBack(idx: number | undefined): boolean {
	return (idx ?? 0) > 0;
}
