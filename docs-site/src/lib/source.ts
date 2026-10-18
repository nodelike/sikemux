import { getCollection, type CollectionEntry } from "astro:content";
import { loader, type StaticSource } from "fumadocs-core/source";
import { relative } from "node:path";

const contentDir = "src/content/docs";

async function collections(): Promise<
  StaticSource<{
    metaData: CollectionEntry<"meta">["data"];
    pageData: CollectionEntry<"docs">["data"] & { entry: CollectionEntry<"docs"> };
  }>
> {
  const pages = (await getCollection("docs")).map((entry) => ({
    type: "page" as const,
    path: relative(contentDir, entry.filePath!),
    data: { ...entry.data, entry },
  }));
  const metas = (await getCollection("meta")).map((entry) => ({
    type: "meta" as const,
    path: relative(contentDir, entry.filePath!),
    data: entry.data,
  }));
  return { files: [...pages, ...metas] };
}

export const source = loader({ source: await collections(), baseUrl: "/" });

export const markdownUrl = (slugs: string[]) => `/${slugs.length > 0 ? slugs.join("/") : "index"}.md`;
