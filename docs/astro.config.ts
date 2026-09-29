import { satteri } from "@astrojs/markdown-satteri";
import starlight from "@astrojs/starlight";
import { defineConfig } from "astro/config";
import starlightLinksValidator from "starlight-links-validator";
import starlightLlmsTxt from "starlight-llms-txt";
import { baseLinks } from "./src/base-links.ts";
import { buildEnv, SITE, SITE_BASE } from "./src/build-env.ts";
import { joinBase } from "./src/versions.ts";

const { versions, current, base, srcDir } = buildEnv;

function llmsDetails(): string | undefined {
	if (current === undefined) return undefined;
	const others = versions
		.filter((v) => v.label !== current.label)
		.map((v) => `${v.label}: ${SITE}${joinBase(SITE_BASE, v.path)}llms.txt`);
	const covers = current.label === "next" ? "the unreleased development version" : `release ${current.label}`;
	return [
		`This file covers ${covers} of pollen.`,
		...(others.length > 0 ? [`Other versions: ${others.join(", ")}.`] : []),
	].join(" ");
}

const details = llmsDetails();

export default defineConfig({
	site: SITE,
	base,
	srcDir,
	markdown: {
		processor: satteri({ mdastPlugins: [baseLinks(base)] }),
	},
	integrations: [
		starlight({
			title: "pollen",
			description:
				"Deploy Agent Skills from git repositories and local directories, declaratively, from a single pollen.yaml.",
			logo: { src: "../assets/logo-icon.svg" },
			favicon: "/favicon.svg",
			social: [{ icon: "github", label: "GitHub", href: "https://github.com/groupbees/pollen" }],
			editLink: { baseUrl: "https://github.com/groupbees/pollen/edit/main/docs/" },
			customCss: ["./src/styles/theme.css"],
			components: {
				Banner: "./src/components/Banner.astro",
				SocialIcons: "./src/components/SocialIcons.astro",
			},
			sidebar: [
				{ label: "Guides", items: [{ autogenerate: { directory: "guides" } }] },
				{ label: "Reference", items: [{ autogenerate: { directory: "reference" } }] },
			],
			plugins: [starlightLinksValidator(), starlightLlmsTxt(details === undefined ? {} : { details })],
		}),
	],
});
