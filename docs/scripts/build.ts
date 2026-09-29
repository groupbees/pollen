// Builds every documentation version into `dist/`: the current site's config,
// theme and components around the pages of each release tag.
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { type Build, ENV_SRC_DIR, ENV_VERSION, ENV_VERSIONS, planBuilds, type Version } from "../src/versions.ts";

const CONTENT_DIR = "docs/src/content/docs";

const docsDir = resolve(import.meta.dirname, "..");
const distDir = join(docsDir, "dist");
const stagingDir = join(docsDir, ".versions");
const astro = join(docsDir, "node_modules", ".bin", "astro");

function log(event: string, fields: Record<string, string | number | boolean> = {}): void {
	const pairs = Object.entries(fields).map(([k, v]) => `${k}=${JSON.stringify(v)}`);
	process.stderr.write(`${[`event=${event}`, ...pairs].join(" ")}\n`);
}

function git(...args: string[]): string {
	return execFileSync("git", args, { cwd: docsDir, encoding: "utf8" }).trim();
}

function hasContent(tag: string): boolean {
	return spawnSync("git", ["cat-file", "-e", `${tag}:${CONTENT_DIR}`], { cwd: docsDir }).status === 0;
}

function stage(build: Build & { ref: string }, repoRoot: string): string {
	const root = join(stagingDir, build.label);
	const srcDir = join(root, "docs", "src");
	cpSync(join(docsDir, "src"), srcDir, { recursive: true });
	cpSync(join(repoRoot, "assets"), join(root, "assets"), { recursive: true });
	rmSync(join(root, CONTENT_DIR), { recursive: true, force: true });
	const archive = execFileSync("git", ["archive", build.ref, CONTENT_DIR], {
		cwd: repoRoot,
		maxBuffer: 512 * 1024 * 1024,
	});
	execFileSync("tar", ["-x", "-C", root], { input: archive });
	return srcDir;
}

function buildVersion(build: Build, versions: Version[], repoRoot: string): void {
	log("version.build.start", { label: build.label, path: build.path, ref: build.ref ?? "working-tree" });
	const env: NodeJS.ProcessEnv = {
		...process.env,
		[ENV_VERSIONS]: JSON.stringify(versions),
		[ENV_VERSION]: build.label,
	};
	if (build.ref !== null) env[ENV_SRC_DIR] = stage({ ...build, ref: build.ref }, repoRoot);

	const outDir = join(stagingDir, "out", build.label);
	// --force: the content-layer cache in node_modules would otherwise serve the
	// previous version's pages.
	const result = spawnSync(astro, ["build", "--force", "--outDir", outDir], { cwd: docsDir, env, stdio: "inherit" });
	if (result.status !== 0) throw new Error(`astro build failed for ${build.label} (exit ${result.status})`);

	cpSync(outDir, join(distDir, build.path), { recursive: true });
	log("version.build.done", { label: build.label });
}

function main(): void {
	const repoRoot = git("rev-parse", "--show-toplevel");
	const tags = git("tag", "--list", "v*").split("\n").filter(Boolean).filter(hasContent);
	const builds = planBuilds(tags);
	const versions: Version[] = builds.map(({ label, path, latest }) => ({ label, path, latest }));
	log("versions.planned", { count: builds.length, labels: builds.map((b) => b.label).join(",") });

	rmSync(distDir, { recursive: true, force: true });
	mkdirSync(distDir, { recursive: true });
	try {
		for (const build of builds) buildVersion(build, versions, repoRoot);
	} finally {
		rmSync(stagingDir, { recursive: true, force: true });
	}
	log("versions.done", { dist: distDir });
}

main();
