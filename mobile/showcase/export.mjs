/*
 * Writes what the showcase's pretend Mac answers as one JSON file, for the iPhone app's showcase mode
 * to read: the device view, the host, its calls, and each chat as attaching it answers.
 *
 *   node showcase/export.mjs <out.json>
 */
import { execFileSync, spawn } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

const here = import.meta.dirname;
const root = resolve(here, '../..');
const out = resolve(process.argv[2] ?? 'showcase.json');

const markdownParser = execFileSync(
  join(root, 'scripts/bin/cargo'),
  ['build', '--quiet', '-p', 'sikemux-markdown', '--example', 'stdio', '--message-format=json'],
  { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 1 << 28 },
)
  .split('\n')
  .filter(Boolean)
  .map((line) => JSON.parse(line))
  .find((message) => message.reason === 'compiler-artifact' && message.target?.name === 'stdio' && message.executable)?.executable;
if (!markdownParser) throw new Error('cargo built no markdown parser');

const parser = spawn(markdownParser, [], { stdio: ['pipe', 'pipe', 'inherit'] });
const waiting = [];
createInterface({ input: parser.stdout }).on('line', (line) => waiting.shift()?.(JSON.parse(line)));
const parse = (text) =>
  new Promise((resolve) => {
    waiting.push(resolve);
    parser.stdin.write(`${JSON.stringify({ text, options: { gfm: true, fileLinks: true } })}\n`);
  });
globalThis.fetch = async (url, request) => {
  if (url !== '/showcase/markdown') throw new Error(`the export fetches nothing but markdown, not ${url}`);
  const trees = await Promise.all(JSON.parse(request.body).map(parse));
  return { json: async () => trees };
};

const work = mkdtempSync(join(tmpdir(), 'sxshowcase'));
try {
  const entry = join(work, 'entry.ts');
  writeFileSync(
    entry,
    `export * from ${JSON.stringify(join(here, 'native.ts'))};\nexport { RECORDED } from ${JSON.stringify(join(here, 'world.ts'))};\n`,
  );
  const bundle = join(work, 'native.mjs');
  execFileSync(
    join(here, '../app/node_modules/.bin/esbuild'),
    [entry, '--bundle', '--platform=node', '--format=esm', `--outfile=${bundle}`, '--log-level=warning'],
    { stdio: 'inherit' },
  );
  const { Device, RECORDED } = await import(pathToFileURL(bundle).href);
  let view;
  const connection = await new Device().connect('', {
    events: (events) => (view ??= JSON.parse(events[0].inner.viewJson)),
    closed() {},
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  const agents = [...view.workspace.agents.map((agent) => agent.id), ...RECORDED.map((chat) => chat.agentId)];
  const chats = {};
  for (const agentId of agents) {
    const attached = await connection.attachChat(agentId);
    if (attached.inner?.viewJson) chats[agentId] = JSON.parse(attached.inner.viewJson);
  }
  const calls = {};
  for (const method of ['questions', 'agentCatalog', 'closedAgents', 'agentSessions', 'codeHostTracked']) {
    calls[method] = JSON.parse(await connection.call(method, '{}'));
  }
  const details = Object.fromEntries(RECORDED.map((chat) => [chat.agentId, chat.recorded.details]));
  const paired = JSON.parse(globalThis.sikemuxTestFiles.get('file:///document/paired-devices.json'));
  writeFileSync(out, JSON.stringify({ view, host: JSON.parse(await connection.host()), calls, chats, details, paired }));
  console.error(`wrote ${out}`);
} finally {
  parser.kill();
  rmSync(work, { recursive: true, force: true });
}
