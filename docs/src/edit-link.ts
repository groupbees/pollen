export const EDIT_BASE_URL = "https://github.com/groupbees/pollen/edit/main/docs/";

const STAGED_VERSION = /^\.versions\/[^/]+\/docs\//;

export function sourcePath(filePath: string): string {
	return filePath.replace(STAGED_VERSION, "");
}

export function editUrl(filePath: string, existsOnMain: (path: string) => boolean): string | undefined {
	const path = sourcePath(filePath);
	return existsOnMain(path) ? EDIT_BASE_URL + path : undefined;
}
