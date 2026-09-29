import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { markdownToHtml } from "satteri";
import { baseLinks, prefixBase } from "./base-links.ts";

describe("prefixBase", () => {
	it("prefixes root-absolute paths only", () => {
		assert.equal(prefixBase("/guides/usage/", "/pollen/next/"), "/pollen/next/guides/usage/");
		assert.equal(prefixBase("https://example.com/", "/pollen/"), "https://example.com/");
		assert.equal(prefixBase("//cdn.example.com/x", "/pollen/"), "//cdn.example.com/x");
		assert.equal(prefixBase("#anchor", "/pollen/"), "#anchor");
		assert.equal(prefixBase("/guides/", "/"), "/guides/");
	});
});

describe("baseLinks", () => {
	it("rewrites inline and reference links", () => {
		const source = "[inline](/guides/usage/) and [ref][r]\n\n[r]: /reference/configuration/\n";
		const { html } = markdownToHtml(source, { mdastPlugins: [baseLinks("/pollen/")] });
		assert.match(html, /href="\/pollen\/guides\/usage\/"/);
		assert.match(html, /href="\/pollen\/reference\/configuration\/"/);
	});

	it("leaves external links alone", () => {
		const { html } = markdownToHtml("[gh](https://github.com/groupbees/pollen)", {
			mdastPlugins: [baseLinks("/pollen/")],
		});
		assert.match(html, /href="https:\/\/github\.com\/groupbees\/pollen"/);
	});
});
