import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { joinBase, parseVersions, planBuilds } from "./versions.ts";

describe("planBuilds", () => {
	it("serves the default branch at the root when there is no release", () => {
		assert.deepEqual(planBuilds([]), [{ label: "next", path: "/", latest: true, ref: null }]);
	});

	it("serves the latest release at the root and the default branch at /next/", () => {
		assert.deepEqual(planBuilds(["v0.1.0"]), [
			{ label: "v0.1", path: "/", latest: true, ref: "v0.1.0" },
			{ label: "next", path: "/next/", latest: false, ref: null },
		]);
	});

	it("keeps the last patch of each minor, newest first", () => {
		assert.deepEqual(planBuilds(["v0.1.0", "v0.2.0", "v0.1.10", "v0.1.2", "v1.0.0"]), [
			{ label: "v1.0", path: "/", latest: true, ref: "v1.0.0" },
			{ label: "next", path: "/next/", latest: false, ref: null },
			{ label: "v0.2", path: "/v0.2/", latest: false, ref: "v0.2.0" },
			{ label: "v0.1", path: "/v0.1/", latest: false, ref: "v0.1.10" },
		]);
	});

	it("ignores pre-releases and tags that are not versions", () => {
		assert.deepEqual(planBuilds(["v0.2.0-rc1", "chart-1.0.0", "v1", "0.1.0"]), [
			{ label: "next", path: "/", latest: true, ref: null },
		]);
	});
});

describe("parseVersions", () => {
	it("reads nothing from an unset variable", () => {
		assert.deepEqual(parseVersions(undefined), []);
		assert.deepEqual(parseVersions(""), []);
	});

	it("reads a version list", () => {
		const versions = [{ label: "next", path: "/next/", latest: false }];
		assert.deepEqual(parseVersions(JSON.stringify(versions)), versions);
	});

	it("rejects a path that is not a directory under the root", () => {
		assert.throws(() => parseVersions(JSON.stringify([{ label: "next", path: "next", latest: false }])));
	});
});

describe("joinBase", () => {
	it("appends a version path to the site base", () => {
		assert.equal(joinBase("/pollen/", "/"), "/pollen/");
		assert.equal(joinBase("/pollen/", "/next/"), "/pollen/next/");
	});
});
