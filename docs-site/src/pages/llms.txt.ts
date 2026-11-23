import type { APIRoute } from "astro";
import type { Node } from "fumadocs-core/page-tree";
import { markdownUrl, source } from "@/lib/source";

/** The pages as plain markdown, listed the way agents look for them: https://llmstxt.org */
export const GET: APIRoute = ({ site }) => {
  const line = (node: Node) => {
    const page = node.type === "page" ? source.getNodePage(node) : undefined;
    if (!page) return [];
    return [`- [${page.data.title}](${new URL(markdownUrl(page.slugs), site)}): ${page.data.description}`];
  };
  const sections = source
    .getPageTree()
    .children.filter((node) => node.type === "folder")
    .map((folder) => `## ${folder.name}\n\n${folder.children.flatMap(line).join("\n")}`);
  const body = `# Sikemux\n\n> A whole team of AI coders, in one window. Official Sikemux documentation (https://sikemux.com · https://github.com/nodelike/sikemux).\n\n${sections.join("\n\n")}\n`;
  return new Response(body, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};
