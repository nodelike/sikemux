// `!important` wins every fight it enters, so each one makes the next rule harder
// to override. The budget only goes down: lower it when you remove one.
import { readdir, readFile } from "node:fs/promises";
import { join, relative, resolve } from "node:path";

const BUDGET = 37;

const root = resolve(import.meta.dirname, "..");
const src = join(root, "src");
const COMMENT = /\/\*[\s\S]*?\*\//gu;
const IMPORTANT = /!\s*important/giu;

async function stylesheets(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(
    entries.map((entry) => {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) return stylesheets(path);
      return entry.name.endsWith(".css") ? [path] : [];
    }),
  );
  return nested.flat();
}

const counts = [];
for (const file of await stylesheets(src)) {
  const text = (await readFile(file, "utf8")).replace(COMMENT, "");
  const count = text.match(IMPORTANT)?.length ?? 0;
  if (count > 0) counts.push([relative(root, file), count]);
}
const total = counts.reduce((sum, [, count]) => sum + count, 0);

if (total > BUDGET) {
  counts.sort((a, b) => b[1] - a[1]);
  console.error(
    [
      `CSS under src/ has ${total} !important, over the budget of ${BUDGET}:`,
      ...counts.map(([file, count]) => `  ${count}\t${file}`),
    ].join("\n"),
  );
  process.exit(1);
}
console.log(`✓ ${total} !important in CSS, within the budget of ${BUDGET}`);
