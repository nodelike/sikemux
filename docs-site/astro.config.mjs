import { unified } from "@astrojs/markdown-remark";
import mdx from "@astrojs/mdx";
import react from "@astrojs/react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "astro/config";
import { auraNoir } from "./src/lib/code-theme.mjs";
import { rehypeKeys } from "./src/lib/rehype-keys.mjs";
import { rehypeCode, remarkHeading, remarkStructure } from "fumadocs-core/mdx-plugins";

export default defineConfig({
  site: "https://docs.sikemux.com",
  markdown: {
    processor: unified({
      syntaxHighlight: false,
      remarkPlugins: [remarkHeading, [remarkStructure, { exportAs: "structuredData" }]],
      rehypePlugins: [[rehypeCode, { themes: { light: auraNoir, dark: auraNoir } }], rehypeKeys],
    }),
  },
  integrations: [react(), mdx({ extendMarkdownConfig: true, syntaxHighlight: false })],
  vite: { plugins: [tailwindcss()] },
});
