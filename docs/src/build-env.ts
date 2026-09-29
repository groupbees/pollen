// The version being built, as `scripts/build.ts` hands it to `astro build`.
// With nothing set, the site is a single version served at the root.
import { ENV_SRC_DIR, ENV_VERSION, ENV_VERSIONS, joinBase, parseVersions, type Version } from "./versions.ts";

export const SITE = "https://groupbees.github.io";
export const SITE_BASE = "/pollen/";

const versions = parseVersions(process.env[ENV_VERSIONS]);
const label = process.env[ENV_VERSION];
const current: Version | undefined = versions.find((v) => v.label === label);

if (label !== undefined && current === undefined) {
	throw new Error(`${ENV_VERSION}=${label} is not in ${ENV_VERSIONS}`);
}

export const buildEnv = {
	versions,
	current,
	base: joinBase(SITE_BASE, current?.path ?? "/"),
	srcDir: process.env[ENV_SRC_DIR] ?? "./src",
};
