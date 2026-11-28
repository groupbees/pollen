import { existsSync } from "node:fs";
import { defineRouteMiddleware } from "@astrojs/starlight/route-data";
import { editUrl } from "./edit-link.ts";

// A released version's pages are staged under the git-ignored `.versions/`, so
// Starlight's own edit link points at a path the repository does not have.
export const onRequest = defineRouteMiddleware(({ locals }) => {
	const route = locals.starlightRoute;
	if (route.entry.data.editUrl !== true) return;
	const url = editUrl(route.entry.filePath, existsSync);
	route.editUrl = url === undefined ? undefined : new URL(url);
});
