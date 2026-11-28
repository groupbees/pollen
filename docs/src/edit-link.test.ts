import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { editUrl, sourcePath } from "./edit-link.ts";

const PAGE = "src/content/docs/guides/usage.md";

describe("sourcePath", () => {
	it("maps a released version's staged page to its path on main", () => {
		assert.equal(sourcePath(`.versions/v0.7/docs/${PAGE}`), PAGE);
	});

	it("keeps the path of a page built from the working tree", () => {
		assert.equal(sourcePath(PAGE), PAGE);
	});
});

describe("editUrl", () => {
	const onMain = new Set([PAGE]);
	const existsOnMain = (path: string) => onMain.has(path);

	it("edits the page on main from a released version", () => {
		assert.equal(
			editUrl(`.versions/v0.7/docs/${PAGE}`, existsOnMain),
			`https://github.com/groupbees/pollen/edit/main/docs/${PAGE}`,
		);
	});

	it("edits the page on main from the working tree", () => {
		assert.equal(editUrl(PAGE, existsOnMain), `https://github.com/groupbees/pollen/edit/main/docs/${PAGE}`);
	});

	it("has no link for a released page that main no longer has", () => {
		assert.equal(editUrl(".versions/v0.1/docs/src/content/docs/guides/removed.md", existsOnMain), undefined);
	});
});
