// The phone and the share page both bundle shared/. Metro cannot load Tauri or the DOM, and neither
// client has the desktop app's code, so this walks everything shared/ imports at runtime and fails
// if any of it reaches a desktop-only package or a file outside shared/ and the server's protocol.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';

import { repo } from './native-build.mjs';

const DESKTOP_ONLY = [/^@tauri-apps\//, /^react-dom(\/|$)/];
const shared = join(repo, 'shared');
const allowed = [shared, join(repo, 'server/protocol')];
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

const files = sourceFiles(shared);
const problems = [];
for (const file of files) {
  for (const specifier of runtimeImports(file)) {
    const from = relative(repo, file);
    if (DESKTOP_ONLY.some((pattern) => pattern.test(specifier))) {
      problems.push(`${from} imports ${specifier}`);
      continue;
    }
    if (!specifier.startsWith('.')) continue;
    const target = resolveModule(resolve(dirname(file), specifier));
    if (!target) problems.push(`${from}: ${specifier} does not resolve to a file`);
    else if (!allowed.some((place) => target.startsWith(`${place}/`))) problems.push(`${from} imports ${relative(repo, target)}, outside shared/`);
  }
}

if (problems.length) {
  console.error(`shared/ reaches code the phone and the share page cannot bundle:\n${problems.join('\n')}`);
  process.exit(1);
}
console.log(`✓ the ${files.length} shared modules import nothing desktop-only`);
