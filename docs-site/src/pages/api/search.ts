import type { APIRoute } from "astro";
import { structure } from "fumadocs-core/mdx-plugins";
import { createFromSource } from "fumadocs-core/search/server";
import { source } from "@/lib/source";

const server = createFromSource(source, {
  buildIndex: (page) => ({
    id: page.url,
    title: page.data.title,
    description: page.data.description,
    url: page.url,
    structuredData: structure(page.data.entry.body ?? ""),
  }),
});

export const GET: APIRoute = () => server.staticGET();
