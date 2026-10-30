import { existsSync, mkdirSync, mkdtempSync, realpathSync, writeFileSync, readFileSync, symlinkSync, cpSync, chmodSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { homedir } from 'node:os';
import { join, dirname } from 'node:path';
import { SHIMMED } from './shims.mjs';
import { REPO, readRepo } from './surface.mjs';

const EVAL = join(REPO, 'evals/tools');
const HOME = homedir();
const PRIVATE = ['proj', 'projects', 'Documents', 'Desktop', 'Downloads', 'Pictures', '.config', '.ssh', '.claude', '.omp', '.codex', '.aws', '.pi', 'nodemem', 'work-context', 'Library/Application Support', 'Library/Messages', 'Library/Mail', 'Library/Mobile Documents'];
const NEVER_RUN = [
  ...['/usr/bin/security', '/bin/launchctl', '/usr/bin/killall', '/usr/bin/pkill'],
  ...['/usr/bin/open', '/usr/bin/osascript', '/usr/sbin/screencapture', '/usr/bin/ssh', '/usr/bin/scp', '/usr/bin/curl', '/usr/bin/make'],
  ...['/usr/bin/xcrun', '/usr/bin/xcodebuild', '/usr/sbin/lsof', '/bin/ps', '/usr/bin/pgrep', '/usr/local/bin/docker'],
  ...['gh', 'aws', 'cargo', 'rustc', 'npm', 'npx', 'pnpm', 'psql', 'wget', 'codex'].map((name) => `/opt/homebrew/bin/${name}`),
  ...['.cargo/bin/cargo', '.cargo/bin/rustc', '.cargo/bin/rustup', '.local/bin/npm', '.local/bin/npx', '.local/bin/vercel'].map((path) => join(HOME, path)),
];
const SIKEMUX_REGEX = ['^/Applications/Sikemux[^/]*\\.app(/|$)', '/Sikemux[^/]*\\.app/Contents/'];
const SIKEMUX_PATHS = [join(REPO, 'target'), '/tmp/sikemux-tool-eval/hosts', '/private/tmp/sikemux-tool-eval/hosts'];
// Other cases' homes, the run folders with every fixture and grade, and other sessions' temporary files.
const OTHERS = [`^/private/tmp/u${'[A-Za-z0-9]'.repeat(6)}(/|$)`, '^/private/tmp/sikemux-tool-eval(/|$)', '^/private/tmp/omphome(/|$)', '^/private/tmp/claude-[0-9]+(/|$)'];
const SIKEMUX_SOCKETS = ['.config/sikemux', 'Library/Application Support/com.nodelike.sikemux'].map((path) => join(HOME, path));
export const AUTHOR = { GIT_AUTHOR_NAME: 'nodelike', GIT_AUTHOR_EMAIL: 'dev@example.com', GIT_COMMITTER_NAME: 'nodelike', GIT_COMMITTER_EMAIL: 'dev@example.com' };

export const BASE_PATH = [
  join(HOME, '.local/bin'),
  join(HOME, '.bun/bin'),
  '/opt/homebrew/bin',
  '/usr/local/bin',
  '/usr/bin',
  '/bin',
  '/usr/sbin',
  '/sbin',
].join(':');

export function prepareCase(dir, testCase, { at = null, host, shell = '/bin/zsh', shellEnv }) {
  const fixture = testCase.fixture ?? {};
  const root = realpathSync(mkdtempSync('/tmp/u'));
  const home = join(root, 'home');
  const repo = join(home, fixture.project === 'work' ? 'work/app' : 'proj/app');
  const bin = join(home, '.local/bin');
  for (const folder of [home, repo, bin, join(home, 'Library'), join(root, 'tmp')]) mkdirSync(folder, { recursive: true });
  symlinkSync(root, join(dir, 'sandbox'));
  writeFileSync(join(home, '.claude.json'), JSON.stringify(claudeAccount()));
  if (shellEnv) for (const name of ['.zshenv', '.bashenv']) cpSync(shellEnv, join(home, name));
  else writeFileSync(join(home, '.zshenv'), `[[ -o interactive ]] || ${agentZshOptions(at)}\n`);
  writeFileSync(join(home, '.zshrc'), shellEnv ? '' : `${agentZshOptions(at)}\n`);
  writeFileSync(join(dir, 'fixture.json'), JSON.stringify(fixture, null, 1));
  const agent = join(dir, 'agent');
  mkdirSync(agent, { recursive: true });
  writeFileSync(join(agent, 'shell-rules.json'), JSON.stringify(fixture.shell ?? []));
  writeFileSync(join(agent, 'rival-rules.json'), JSON.stringify((fixture.answers ?? []).filter((rule) => [rule.tool].flat().some((name) => name.includes('.')))));

  writeFileSync(join(bin, 'timeout'), TIMEOUT);
  chmodSync(join(bin, 'timeout'), 0o755);
  for (const name of Object.keys(SHIMMED)) {
    const shim = join(bin, name);
    writeFileSync(shim, `#!/bin/sh\nexec "${process.execPath}" "${join(EVAL, 'lib/shim.mjs')}" ${name} "$@"\n`);
    chmodSync(shim, 0o755);
  }

  const work = fixture.project === 'work';
  cpSync(join(EVAL, 'fixtures/project', work ? 'work' : 'sikemux'), repo, { recursive: true });
  if (!work) for (const real of ['AGENTS.md', 'sikemux.json']) writeFileSync(join(repo, real), readRepo(real, at));
  const instructions = [readFileSync(join(repo, 'AGENTS.md'), 'utf8'), fixture.instructions].filter(Boolean).join('\n');
  writeFileSync(join(repo, 'AGENTS.md'), instructions);
  writeFileSync(join(repo, 'CLAUDE.md'), instructions);
  writeFiles(repo, fixture.files);
  const git = (...args) => execFileSync('/usr/bin/git', args, { cwd: repo, env: { ...AUTHOR, HOME: home, PATH: BASE_PATH }, stdio: 'ignore' });
  git('init', '-q', '-b', 'main');
  git('add', '-A');
  git('commit', '-q', '-m', 'Start the project');
  for (const commit of fixture.commits ?? []) {
    writeFiles(repo, commit.files);
    git('add', '-A');
    git('commit', '-q', '-m', commit.message);
  }
  if (fixture.remote) git('remote', 'add', 'origin', fixture.remote);
  for (const branch of fixture.branches ?? []) git('branch', branch);
  writeFiles(repo, fixture.dirty);

  writeFileSync(join(dir, 'tools.sb'), profile([dir, root], host));
  writeFileSync(join(dir, 'preview.sb'), profile([dir, root], null, { root, agent }));
  return {
    root,
    agent,
    home,
    repo,
    env: {
      ...AUTHOR,
      HOME: home,
      USER: process.env.USER,
      LOGNAME: process.env.USER,
      PATH: `${bin}:${BASE_PATH}`,
      SHELL: shell,
      CLAUDE_CODE_SHELL: shell,
      ...(shellEnv ? { BASH_ENV: join(home, '.bashenv') } : {}),
      LANG: 'en_US.UTF-8',
      TERM: 'dumb',
      TMPDIR: join(root, 'tmp/'),
      TOOL_EVAL_CASE: dir,
    },
  };
}

function writeFiles(root, files = {}) {
  for (const [path, content] of Object.entries(files)) {
    const target = join(root, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, content);
  }
}

function claudeAccount() {
  const config = JSON.parse(readFileSync(join(HOME, '.claude.json'), 'utf8'));
  const kept = Object.fromEntries(['oauthAccount', 'userID', 'hasCompletedOnboarding'].filter((key) => key in config).map((key) => [key, config[key]]));
  return { ...kept, bypassPermissionsModeAccepted: true };
}

// The agent's profile keeps every Sikemux program out of reach: one started with the sandbox's home once replaced the person's host key.
// The tools profile is the same without that, for the one sikemux-host the harness itself runs.
// The zsh options Sikemux starts agents' shells with.
function agentZshOptions(at) {
  return readRepo('crates/sikemux-core/src/agent_shell.rs', at).match(/const OPTIONS: &str = "([^"]+)"/)[1];
}

// Sikemux links a GNU-style timeout beside its git shim; macOS has none.
const TIMEOUT = `#!/bin/sh
while [ $# -gt 0 ]; do
  case "$1" in
    -k|-s|--kill-after|--signal) shift 2 ;;
    --) shift; break ;;
    -*) shift ;;
    *) break ;;
  esac
done
exec /usr/bin/perl -e '
  my $limit = shift; my %unit = (s => 1, m => 60, h => 3600, d => 86400);
  $limit =~ /^([0-9.]+)([smhd]?)$/ or die "timeout: invalid time interval\n";
  my $seconds = $1 * ($unit{$2 || "s"});
  my $child = fork; if (!$child) { exec @ARGV or exit 127 }
  local $SIG{ALRM} = sub { kill "TERM", $child; waitpid $child, 0; exit 124 };
  alarm $seconds; waitpid $child, 0; alarm 0; exit($? >> 8);
' "$@"
`;

// Written once the relay's port is known: the agent reaches only that one local port.
export function writeAgentProfile(dir, sandbox, relayPort) {
  writeFileSync(join(dir, 'sandbox.sb'), profile([dir, sandbox.root], null, { root: sandbox.root, agent: sandbox.agent, relayPort }));
}

function profile(writable, host, own) {
  const quote = (path) => JSON.stringify(path);
  const sikemux = [...SIKEMUX_REGEX.map((pattern) => `(regex #"${pattern}")`), ...SIKEMUX_PATHS.map((path) => `(subpath ${quote(path)})`)].join(' ');
  return [
    '(version 1)',
    '(allow default)',
    `(deny file-write* (subpath ${quote(HOME)}))`,
    `(allow file-write* ${writable.map((path) => `(subpath ${quote(path)})`).join(' ')})`,
    `(deny file-read* ${PRIVATE.map((path) => `(subpath ${quote(join(HOME, path))})`).join(' ')})`,
    '(allow file-read-metadata)',
    `(allow file-read* (subpath ${quote(join(EVAL, 'lib'))}) (subpath ${quote(join(EVAL, 'rivals'))}))`,
    `(deny process-exec ${[...new Set(NEVER_RUN.flatMap((path) => (existsSync(path) ? [path, realpathSync(path)] : [])))].map((path) => `(literal ${quote(path)})`).join(' ')})`,
    `(deny network-outbound ${SIKEMUX_SOCKETS.map((path) => `(remote unix-socket (path-regex #"^${path.replace(/[.]/g, '\\.')}/"))`).join(' ')})`,
    '(deny signal)',
    '(allow signal (target same-sandbox))',
    ...(host ? [`(allow file-read* process-exec (literal ${quote(host)}))`] : [`(deny file-read* process-exec ${sikemux})`]),
    ...(own
      ? [
          `(deny file-read-data (literal ${quote(HOME)}))`,
          `(deny process-exec (subpath "/Applications") (subpath ${quote(join(HOME, 'Applications'))}))`,
          '(deny network-outbound (remote ip "localhost:*"))',
          ...(own.relayPort ? [`(allow network-outbound (remote ip "localhost:${own.relayPort}"))`] : []),
        ]
      : []),
    ...(own ? [`(deny file-read* ${OTHERS.map((pattern) => `(regex #"${pattern}")`).join(' ')})`, `(allow file-read* (subpath ${quote(own.root)}) (subpath ${quote(own.agent)}) (regex #"^/private/tmp/claude-[0-9]+/${own.root.replace(/[^A-Za-z0-9]/g, '-')}-") (literal ${quote(`/private/tmp/claude-${process.getuid()}`)}))`] : []),
    '',
  ].join('\n');
}
