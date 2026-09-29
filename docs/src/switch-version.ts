export function switchVersion(pathname: string, currentBase: string, targetBase: string): string {
	if (!pathname.startsWith(currentBase)) return targetBase;
	return targetBase + pathname.slice(currentBase.length);
}
