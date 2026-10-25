// Lists the files the phone's JavaScript bundle is built from, the way Metro follows them: index.ts and
// every route, then their imports through the app's path aliases. Packages are left out.
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { build } from 'esbuild';

const app = resolve(import.meta.dirname, '..');
const repo = resolve(app, '../..');
const OWN = /^(\.|\/|@\/|@shared\/|@protocol(\/|$))/;

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

/** Every file in the repository the bundle reads, relative to the repository root. */
export async function bundleInputs() {
  const routes = files(resolve(app, 'src/app')).filter((path) => /\.tsx?$/.test(path) && !/\.test\.tsx?$/.test(path));
  const result = await build({
    entryPoints: [resolve(app, 'index.ts'), ...routes],
    absWorkingDir: repo,
    bundle: true,
    write: false,
    metafile: true,
    logLevel: 'silent',
    outdir: resolve(app, 'node_modules/.cache/bundle-inputs'),
    platform: 'neutral',
    format: 'esm',
    jsx: 'automatic',
    loader: { '.png': 'empty', '.jpg': 'empty', '.svg': 'empty', '.ttf': 'empty', '.otf': 'empty' },
    resolveExtensions: ['.tsx', '.ts', '.jsx', '.js', '.mjs', '.json'],
    alias: {
      '@': resolve(app, 'src'),
      '@shared': resolve(repo, 'shared'),
      '@protocol': resolve(repo, 'server/protocol/generated/types.ts'),
    },
    plugins: [
      {
        name: 'packages',
        setup(builder) {
          builder.onResolve({ filter: /^@protocol\// }, (args) => ({
            path: resolve(repo, 'server/protocol/src', `${args.path.slice('@protocol/'.length)}.ts`),
          }));
          builder.onResolve({ filter: /.*/ }, (args) => (OWN.test(args.path) ? undefined : { path: args.path, external: true }));
        },
      },
    ],
  });
  return Object.keys(result.metafile.inputs)
    .map((input) => relative(repo, resolve(repo, input)))
    .sort();
}

/** The `paths` a push to main must touch for the update workflow to publish. */
export function updateWorkflowPaths() {
  const workflow = readFileSync(resolve(repo, '.github/workflows/mobile-update.yml'), 'utf8');
  const block = /^ {4}paths:\n((?: {6}- .*\n)+)/m.exec(workflow)?.[1] ?? '';
  return [...block.matchAll(/^ {6}- "?([^"\n]+)"?$/gm)].map((match) => match[1]);
}

/** Whether `path` matches a GitHub Actions path filter: `**` crosses folders, `*` does not. */
export function matchesFilter(path, filter) {
  const pattern = filter
    .split(/(\*\*\/?|\*)/)
    .map((part) => (part.startsWith('**') ? '.*' : part === '*' ? '[^/]*' : part.replace(/[.+?^${}()|[\]\\]/g, '\\$&')))
    .join('');
  return new RegExp(`^${pattern}$`).test(path);
}
