import { glob } from "astro/loaders";
import { defineCollection, z } from "astro:content";

const base = "./src/content/docs";

export const collections = {
  docs: defineCollection({
    loader: glob({ pattern: "**/*.{md,mdx}", base }),
    schema: z.object({ title: z.string(), description: z.string() }),
  }),
  meta: defineCollection({
    loader: glob({ pattern: "**/meta.json", base }),
    schema: z.object({
      title: z.string().optional(),
      defaultOpen: z.boolean().optional(),
      pages: z.array(z.string()),
    }),
  }),
};
