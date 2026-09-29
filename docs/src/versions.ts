import { z } from "zod";

export const ENV_VERSIONS = "POLLEN_DOCS_VERSIONS";
export const ENV_VERSION = "POLLEN_DOCS_VERSION";
export const ENV_SRC_DIR = "POLLEN_DOCS_SRC_DIR";

export const versionSchema = z.object({
	label: z.string().min(1),
	path: z.string().regex(/^\/([a-z0-9.-]+\/)?$/),
	latest: z.boolean(),
});

export type Version = z.infer<typeof versionSchema>;

export interface Build extends Version {
	/** The tag whose pages this version serves, or `null` for the working tree. */
	ref: string | null;
}

const STABLE_TAG = /^v(\d+)\.(\d+)\.(\d+)$/;

interface Release {
	tag: string;
	major: number;
	minor: number;
	patch: number;
}

function parseRelease(tag: string): Release | null {
	const match = STABLE_TAG.exec(tag);
	if (!match) return null;
	const [, major, minor, patch] = match;
	return { tag, major: Number(major), minor: Number(minor), patch: Number(patch) };
}

function newestFirst(a: Release, b: Release): number {
	return b.major - a.major || b.minor - a.minor || b.patch - a.patch;
}

export function planBuilds(tags: readonly string[]): Build[] {
	const lastPatchPerMinor = new Map<string, Release>();
	for (const release of tags
		.map(parseRelease)
		.filter((r) => r !== null)
		.sort(newestFirst)) {
		const minor = `v${release.major}.${release.minor}`;
		if (!lastPatchPerMinor.has(minor)) lastPatchPerMinor.set(minor, release);
	}

	const [latest, ...older] = [...lastPatchPerMinor.entries()];
	if (!latest) return [{ label: "next", path: "/", latest: true, ref: null }];

	return [
		{ label: latest[0], path: "/", latest: true, ref: latest[1].tag },
		{ label: "next", path: "/next/", latest: false, ref: null },
		...older.map(([label, release]) => ({ label, path: `/${label}/`, latest: false, ref: release.tag })),
	];
}

export function parseVersions(raw: string | undefined): Version[] {
	if (raw === undefined || raw === "") return [];
	return z.array(versionSchema).parse(JSON.parse(raw));
}

export function joinBase(siteBase: string, path: string): string {
	return siteBase.replace(/\/$/, "") + path;
}
