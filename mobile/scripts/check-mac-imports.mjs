// The phone bundles some of the Mac app's modules through `@mac/...`. Metro cannot
// load Tauri or the DOM, so this walks everything those modules import at runtime
// and fails if any of it reaches a desktop-only package.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';

import { mobile, repo } from './native-build.mjs';

const DESKTOP_ONLY = [/^@tauri-apps\//, /^react-dom(\/|$)/];
const appSrc = join(mobile, 'app/src');
const macSrc = join(repo, 'src');
const IMPORT =
  /(?:^|\n)\s*(import|export)\s+(type\s+)?[^;]*?from\s+["']([^"']+)["']|(?:^|\n)\s*import\s+["']([^"']+)["']|import\(\s*["']([^"']+)["']\s*\)/g;

function sourceFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name) ? [path] : [];
  });
}

function runtimeImports(file) {
  const text = readFileSync(file, 'utf8');
  return [...text.matchAll(IMPORT)].filter((match) => !match[2]).map((match) => match[3] ?? match[4] ?? match[5]);
}

function resolveModule(base) {
  const candidates = [base, `${base}.ts`, `${base}.tsx`, join(base, 'index.ts'), join(base, 'index.tsx')];
  return candidates.find((path) => existsSync(path) && statSync(path).isFile());
}

const reachedFrom = new Map();
const queue = [];
for (const file of sourceFiles(appSrc)) {
  for (const specifier of runtimeImports(file)) {
    if (!specifier.startsWith('@mac/')) continue;
    const target = resolveModule(join(macSrc, specifier.slice('@mac/'.length)));
    if (!target) {
      console.error(`${relative(repo, file)}: ${specifier} does not resolve to a file in src/`);
      process.exit(1);
    }
    if (!reachedFrom.has(target)) {
      reachedFrom.set(target, file);
      queue.push(target);
    }
  }
}

const problems = [];
while (queue.length) {
  const file = queue.shift();
  for (const specifier of runtimeImports(file)) {
    if (DESKTOP_ONLY.some((pattern) => pattern.test(specifier))) {
      const chain = [file];
      while (reachedFrom.has(chain[0])) chain.unshift(reachedFrom.get(chain[0]));
      problems.push(`${specifier} is imported by ${chain.map((path) => relative(repo, path)).join(' -> ')}`);
      continue;
    }
    if (!specifier.startsWith('.')) continue;
    const target = resolveModule(resolve(dirname(file), specifier));
    if (target && !reachedFrom.has(target)) {
      reachedFrom.set(target, file);
      queue.push(target);
    }
  }
}

if (problems.length) {
  console.error(`The phone imports Mac code that reaches desktop-only packages, which Metro cannot bundle:\n${problems.join('\n')}`);
  process.exit(1);
}
console.log(`✓ the ${reachedFrom.size} Mac modules the phone bundles import nothing desktop-only`);
