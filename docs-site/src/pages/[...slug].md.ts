import type { APIRoute, GetStaticPaths } from "astro";
import { markdownUrl, source } from "@/lib/source";

/** Every page again as plain markdown, at its own address with `.md` on the end. */
export const getStaticPaths: GetStaticPaths = () =>
  source.getPages().map((page) => ({
    params: { slug: markdownUrl(page.slugs).slice(1, -".md".length) },
    props: { title: page.data.title, body: page.data.entry.body ?? "" },
  }));

export const GET: APIRoute<{ title: string; body: string }> = ({ props: { title, body } }) => {
  const text = body.replace(/\{\/\*[\s\S]*?\*\/\}\n*/g, "").trim();
  return new Response(`# ${title}\n\n${text}\n`, { headers: { "Content-Type": "text/markdown; charset=utf-8" } });
};
