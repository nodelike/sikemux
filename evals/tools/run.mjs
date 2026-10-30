#!/usr/bin/env node
import { parseArgs } from 'node:util';
import { readFileSync, readdirSync, mkdirSync, writeFileSync, realpathSync, statSync, existsSync, copyFileSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { loadSurface, REPO, SIGNED_IN_PLUGINS } from './lib/surface.mjs';
import { startCore } from './lib/core.mjs';
import { prepareCase, writeAgentProfile } from './lib/sandbox.mjs';
import { MODELS, mcpServers, runClaude, readClaude, killAll } from './lib/runners.mjs';
import { startTools } from './lib/tools.mjs';
import { runOmp, readOmp } from './lib/omp.mjs';
import { grade } from './lib/grade.mjs';
import { judge, readVerdict } from './lib/judge.mjs';
import { report } from './lib/report.mjs';

const EVAL = join(REPO, 'evals/tools');
const SURFACE = ['browser', 'crates/sikemux-host/src/tools_mcp', 'plugins', 'AGENTS.md'];
const BUILT_HOST = join(REPO, 'target/debug/sikemux-host');
const HOSTS = '/tmp/sikemux-tool-eval/hosts';

const { values: options } = parseArgs({
  options: {
    models: { type: 'string', default: 'sonnet' },
    search: { type: 'string', default: 'on' },
    cases: { type: 'string' },
    'cases-dir': { type: 'string' },
    'results-dir': { type: 'string' },
    category: { type: 'string' },
    concurrency: { type: 'string', default: '4' },
    turns: { type: 'string', default: '16' },
    timeout: { type: 'string', default: '420' },
    build: { type: 'boolean', default: false },
    at: { type: 'string' },
    resume: { type: 'string' },
    keep: { type: 'boolean', default: false },
    trials: { type: 'string', default: '1' },
    shell: { type: 'string', default: '/bin/zsh' },
    'shell-env': { type: 'string' },
    record: { type: 'string' },
    list: { type: 'boolean', default: false },
    help: { type: 'boolean', short: 'h', default: false },
  },
});

if (options.help) {
  console.log(`pnpm eval:tools [options]
  --models sonnet,opus,deepseek   models to run (also haiku)
  --search on|off|both            Claude tool search; DeepSeek ignores it
  --cases id,id,prefix*           cases to run (default: every replayable case)
  --category wrong_tool,...       only these categories
  --concurrency 4                 cases at once per model
  --trials 1                      runs of each case; reports the mean pass rate and the cases that pass every trial
  --turns 16 --timeout 420        limits per case (turns, seconds)
  --build                         rebuild sikemux-host first
  --shell /bin/bash               the shell agents' commands run in (default /bin/zsh)
  --shell-env <file>              startup lines that shell reads first, as ~/.zshenv or BASH_ENV
  --keep                          keep each case's home and repo after grading
  --resume <run folder>           finish a run that stopped: grade what finished, run the rest
  --at <commit>                   run the tools as they were at a commit, with its host saved by an earlier build
  --cases-dir <folder>            read cases from another folder, such as a held-back set kept outside the repo
  --results-dir <folder>          where --record saves the result (default evals/tools/results)
  --record <label>                also save the result under evals/tools/results/
  --list                          list the cases that would run, and stop`);
  process.exit(0);
}

const cases = loadCases();
if (options.list) {
  for (const testCase of cases) console.log(`${testCase.id.padEnd(28)} ${testCase.category.padEnd(15)} ${testCase.prompt.slice(0, 90).replace(/\n/g, ' ')}`);
  console.log(`${cases.length} cases`);
  process.exit(0);
}

const roots = new Set();
const stopEverything = () => {
  killAll();
  for (const root of roots) sweep(root);
};
process.on('exit', stopEverything);
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => {
    stopEverything();
    process.exit(130);
  });
}
process.on('uncaughtException', (error) => {
  stopEverything();
  console.error(error);
  process.exit(1);
});

const at = options.at ? git('rev-parse', '--short=9', options.at) : null;
const HOST = at ? savedHost(at) : ensureHost();
const variants = options.models.split(',').flatMap((alias) => {
  const model = MODELS[alias];
  if (!model) throw new Error(`no model called ${alias}; pick from ${Object.keys(MODELS).join(', ')}`);
  if (model.runner !== 'claude') return [{ label: alias, alias, ...model }];
  const searches = options.search === 'both' ? [true, false] : [options.search !== 'off'];
  return searches.map((search) => ({ label: `${alias}${search ? '' : ' (no tool search)'}`, alias, search, ...model }));
});

mkdirSync('/tmp/sikemux-tool-eval', { recursive: true });
const earlier = options.resume ? recordedFailures(options.resume) : new Map();
const runDir = options.resume ? realpathSync(options.resume) : join(realpathSync('/tmp/sikemux-tool-eval'), new Date().toISOString().replace(/[:.]/g, '-'));
mkdirSync(runDir, { recursive: true });
console.log(`${cases.length} cases × ${variants.map((variant) => variant.label).join(', ')}; runs in ${runDir}`);

const started = Date.now();
const results = await Promise.all(variants.map((variant) => runVariant(variant)));
const summary = {
  date: new Date().toISOString(),
  commit: at ? git('rev-parse', at) : git('rev-parse', 'HEAD'),
  dirty: !at && git('status', '--porcelain', '--', ...SURFACE) !== '',
  shell: options.shell,
  ...(options['shell-env'] ? { shellEnv: readFileSync(options['shell-env'], 'utf8') } : {}),
  claude: execFileSync(join(process.env.HOME, '.local/bin/claude'), ['--version'], { encoding: 'utf8' }).trim(),
  seconds: Math.round((Date.now() - started) / 1000),
  variants: results,
};
writeFileSync(join(runDir, 'result.json'), JSON.stringify(summary, null, 1));
report(summary, cases);
console.log(`\nresult: ${join(runDir, 'result.json')}`);
if (options.record) {
  const day = summary.date.slice(0, 10);
  const folder = options['results-dir'] ?? join(EVAL, 'results');
  mkdirSync(folder, { recursive: true });
  const path = join(folder, `${day}-${summary.commit.slice(0, 9)}-${options.record}.json`);
  writeFileSync(path, `${JSON.stringify(compact(summary), null, 1)}\n`);
  console.log(`recorded: ${path}`);
}

function loadCases() {
  const folder = options['cases-dir'] ?? join(EVAL, 'cases');
  const all = readdirSync(folder)
    .filter((name) => name.endsWith('.json'))
    .flatMap((name) => JSON.parse(readFileSync(join(folder, name), 'utf8')).map((testCase) => ({ ...testCase, file: name })));
  const ids = new Set();
  for (const testCase of all) {
    if (ids.has(testCase.id)) throw new Error(`two cases are called ${testCase.id}`);
    ids.add(testCase.id);
  }
  const wanted = options.cases?.split(',');
  const categories = options.category?.split(',');
  return all.filter(
    (testCase) =>
      !testCase.skip &&
      (!wanted || wanted.some((id) => (id.endsWith('*') ? testCase.id.startsWith(id.slice(0, -1)) : testCase.id === id))) &&
      (!categories || categories.includes(testCase.category)),
  );
}

function ensureHost() {
  const sources = join(REPO, 'crates/sikemux-host/src/tools_mcp');
  const newest = Math.max(...readdirSync(sources).map((name) => statSync(join(sources, name)).mtimeMs));
  if (options.build || !existsSync(BUILT_HOST) || statSync(BUILT_HOST).mtimeMs < newest) {
    console.log('building sikemux-host…');
    execFileSync('cargo', ['build', '--manifest-path', join(REPO, 'Cargo.toml'), '-p', 'sikemux-host', '--bin', 'sikemux-host'], {
      stdio: 'inherit',
      env: { ...process.env, PATH: `${join(process.env.HOME, '.cargo/bin')}:${process.env.PATH}` },
    });
  }
  if (git('status', '--porcelain', '--untracked-files=no', '--', 'crates', 'plugins', 'Cargo.toml', 'Cargo.lock') === '') {
    const saved = join(HOSTS, git('rev-parse', '--short=9', 'HEAD'));
    mkdirSync(saved, { recursive: true });
    copyFileSync(BUILT_HOST, join(saved, 'sikemux-host'));
  }
  return BUILT_HOST;
}

function savedHost(commit) {
  const path = join(HOSTS, commit, 'sikemux-host');
  if (!existsSync(path)) throw new Error(`no host saved for ${commit}: check it out and run once to build and save it`);
  return path;
}

async function runVariant(variant) {
  const trials = Number(options.trials);
  const queue = cases.flatMap((testCase) => Array.from({ length: trials }, (_, trial) => ({ testCase, trial: trial + 1 })));
  const done = [];
  const workers = Array.from({ length: Number(options.concurrency) }, async () => {
    while (queue.length) {
      const { testCase, trial } = queue.shift();
      const result = await runCase(variant, testCase, trials > 1 ? trial : null);
      done.push(result);
      const mark = result.pass ? 'pass' : 'FAIL';
      const name = result.trial ? `${testCase.id} #${result.trial}` : testCase.id;
      console.log(`${variant.label.padEnd(26)} ${mark} ${name}${result.pass ? '' : `  ${result.failures.map((failure) => failure.why).join('; ')}`}`);
    }
  });
  await Promise.all(workers);
  const byCase = Map.groupBy(done, (result) => result.id);
  const rows = cases.map((testCase) => (trials > 1 ? summarizeTrials(byCase.get(testCase.id)) : byCase.get(testCase.id)[0]));
  rows.sort((left, right) => left.id.localeCompare(right.id));
  return { label: variant.label, model: variant.id, search: variant.search ?? null, trials, cases: rows };
}

function summarizeTrials(runs) {
  runs.sort((left, right) => left.trial - right.trial);
  const passed = runs.filter((run) => run.pass).length;
  const { id, category, tools } = runs[0];
  return {
    id,
    category,
    tools,
    pass: passed === runs.length,
    rate: passed / runs.length,
    failures: runs.flatMap((run) => run.failures.map((failure) => ({ ...failure, trial: run.trial }))),
    cost: runs.reduce((sum, run) => sum + run.cost, 0),
    ...(runs.some((run) => run.costEstimated) ? { costEstimated: true } : {}),
    tokens: {
      input: runs.reduce((sum, run) => sum + run.tokens.input, 0),
      cached: runs.reduce((sum, run) => sum + run.tokens.cached, 0),
      output: runs.reduce((sum, run) => sum + run.tokens.output, 0),
    },
    runs,
  };
}

async function runCase(variant, testCase, trial) {
  const caseDir = join(runDir, variant.label.replace(/\W+/g, '-'), testCase.id);
  const dir = trial ? join(caseDir, `t${trial}`) : caseDir;
  const finished = options.resume ? readFinished(variant, testCase, dir) : null;
  const outcome = finished ?? (await runAgent(variant, testCase, dir));
  const failures = grade(testCase, outcome, repoOf(dir, testCase), earlier.get(`${variant.label}\0${testCase.id}\0${trial ?? ''}`));
  if (outcome.exit.timedOut) failures.push({ check: 'run', why: 'timed out' });
  else if (outcome.failed) failures.push({ check: 'run', why: `the run failed: ${outcome.stop}` });
  const verdict = testCase.judge && !outcome.failed ? await judgeOnce(testCase, outcome, dir, finished ? null : outcome.sandbox) : null;
  dropSandbox(dir);
  if (verdict && !verdict.pass) failures.push({ check: 'judge', why: verdict.why });
  return {
    id: testCase.id,
    ...(trial ? { trial } : {}),
    category: testCase.category,
    tools: testCase.tools ?? [],
    pass: failures.length === 0,
    failures,
    judge: verdict,
    cost: outcome.cost + (verdict?.cost ?? 0),
    ...(outcome.costEstimated ? { costEstimated: true } : {}),
    tokens: outcome.tokens,
    turns: outcome.turns,
    calls: outcome.calls.map((call) => ({ tool: call.tool, args: call.args, error: call.error ?? false, batch: call.batch, sub: call.sub || undefined })),
    reply: outcome.reply?.slice(0, 2000) ?? '',
    dir,
  };
}

// What a run already graded, so a resumed run keeps the file checks it can no longer make once the repos are gone.
function recordedFailures(folder) {
  const path = join(folder, 'result.json');
  const recorded = new Map();
  if (!existsSync(path)) return recorded;
  for (const variant of JSON.parse(readFileSync(path, 'utf8')).variants) {
    for (const row of variant.cases) {
      for (const run of row.runs ?? [row]) recorded.set(`${variant.label}\0${row.id}\0${run.trial ?? ''}`, run.failures);
    }
  }
  return recorded;
}

function repoOf(dir, testCase) {
  const link = join(dir, 'sandbox');
  if (!existsSync(link)) return null;
  return join(realpathSync(link), 'home', testCase.fixture?.project === 'work' ? 'work/app' : 'proj/app');
}

function readFinished(variant, testCase, dir) {
  const transcript = join(dir, 'transcript.jsonl');
  if (!existsSync(transcript)) return null;
  const outcome = variant.runner === 'claude' ? readClaude(transcript) : readOmp(transcript, ['sikemux-tools', ...(testCase.fixture?.rivals ?? [])]);
  const ended = variant.runner === 'claude' ? outcome.stop !== 'no result' : !outcome.failed;
  return ended ? { ...outcome, exit: { timedOut: false } } : null;
}

async function judgeOnce(testCase, outcome, dir, sandbox) {
  const given = options.resume ? readVerdict(join(dir, 'judge')) : null;
  if (given) return given;
  if (sandbox) return judge({ testCase, outcome, dir, sandbox });
  const again = join(dir, 'rejudge');
  rmSync(again, { recursive: true, force: true });
  mkdirSync(again, { recursive: true });
  const fresh = prepareCase(again, testCase, { at, host: HOST });
  writeAgentProfile(again, fresh, null);
  const verdict = await judge({ testCase, outcome, dir: again, sandbox: fresh });
  dropSandbox(again);
  return verdict;
}

function sweep(root) {
  try {
    execFileSync('/usr/bin/pkill', ['-9', '-f', root], { stdio: 'ignore' });
  } catch {}
}

function dropSandbox(dir) {
  const link = join(dir, 'sandbox');
  if (options.keep || !existsSync(link)) return;
  rmSync(realpathSync(link), { recursive: true, force: true });
}

async function runAgent(variant, testCase, dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const sandbox = prepareCase(dir, testCase, { at, host: HOST, shell: options.shell, shellEnv: options['shell-env'] });
  const fixture = testCase.fixture ?? {};
  const surface = loadSurface(fixture.plugins ?? SIGNED_IN_PLUGINS, at);
  const endpoint = join(dir, 'endpoint.json');
  const core = await startCore({ surface, fixture, project: sandbox.repo, endpointFile: endpoint, logFile: join(dir, 'core.jsonl'), previewProfile: join(dir, 'preview.sb') });
  roots.add(sandbox.root);
  const tools = await startTools({ host: HOST, endpoint, sandbox, profile: join(dir, 'tools.sb') });
  writeAgentProfile(dir, sandbox, tools.port);
  const servers = mcpServers({ tools: tools.config, dir, rivals: fixture.rivals });
  const limits = { maxTurns: testCase.maxTurns ?? Number(options.turns), timeoutMs: Number(options.timeout) * 1000 };
  let outcome;
  try {
    outcome =
      variant.runner === 'claude'
        ? await runClaude({ model: variant.id, search: variant.search, dir, sandbox, servers, prompt: testCase.prompt, ...limits })
        : await runOmp({ model: variant.id, dir, sandbox, servers, prompt: testCase.prompt, ...limits });
  } finally {
    tools.close();
    core.close();
    sweep(sandbox.root);
  }
  return { ...outcome, sandbox };
}

function compact(result) {
  return {
    ...result,
    variants: result.variants.map((variant) => ({
      ...variant,
      cases: variant.cases.map((row) => (row.runs ? { ...row, runs: row.runs.map(compactRun) } : compactRun(row))),
    })),
  };
}

function compactRun({ dir, calls, reply, ...rest }) {
  return { ...rest, calls: calls.map((call) => call.tool) };
}

function git(...args) {
  return execFileSync('git', args, { cwd: REPO, encoding: 'utf8' }).trim();
}
