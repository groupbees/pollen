import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { switchVersion } from "./switch-version.ts";

describe("switchVersion", () => {
	it("keeps the page when moving to another version", () => {
		assert.equal(switchVersion("/pollen/guides/usage/", "/pollen/", "/pollen/next/"), "/pollen/next/guides/usage/");
		assert.equal(switchVersion("/pollen/next/guides/usage/", "/pollen/next/", "/pollen/"), "/pollen/guides/usage/");
	});

	it("lands on the target's home page from outside the current base", () => {
		assert.equal(switchVersion("/elsewhere/", "/pollen/next/", "/pollen/v0.1/"), "/pollen/v0.1/");
	});
});
