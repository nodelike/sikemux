import { spawn, execFileSync } from 'node:child_process';
import { createWriteStream, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { REPO, readRepo } from './surface.mjs';

const EVAL = join(REPO, 'evals/tools');
const SERVER_PREFIX = /^mcp__([^_]+(?:[-_][^_]+)*?)__(.+)$/;

export const MODELS = {
  sonnet: { runner: 'claude', id: 'claude-sonnet-5-5' },
  opus: { runner: 'claude', id: 'claude-opus-5-5' },
  haiku: { runner: 'claude', id: 'claude-haiku-5-5' },
  deepseek: { runner: 'omp', id: 'deepseek-ai/DeepSeek-V4.1-Flash-ultra-chat' },
};

export function mcpServers({ tools, dir, rivals = [] }) {
  const servers = { 'sikemux-tools': tools };
  for (const rival of rivals) {
    servers[rival] = { command: process.execPath, args: [join(EVAL, 'lib/rival.mjs'), join(EVAL, `rivals/${rival.toLowerCase()}.json`), dir] };
  }
  return { mcpServers: servers };
}

const groups = new Set();

export function killAll() {
  for (const group of groups) stopGroup(group, 'SIGKILL');
}

function stopGroup(group, signal) {
  try {
    process.kill(-group, signal);
  } catch {
    groups.delete(group);
  }
}

// The Claude sign-in is read fresh for each run and handed over in the environment, so no sandbox ever touches the keychain.
export function claudeToken() {
  const stored = execFileSync('/usr/bin/security', ['find-generic-password', '-s', 'Claude Code-credentials', '-a', process.env.USER, '-w'], { encoding: 'utf8' });
  return JSON.parse(stored).claudeAiOauth.accessToken;
}

export function run({ command, args, cwd, env, dir, timeoutMs, profile = join(dir, 'sandbox.sb') }) {
  return new Promise((resolve) => {
    const out = createWriteStream(join(dir, 'transcript.jsonl'));
    const err = createWriteStream(join(dir, 'stderr.log'));
    const child = spawn('/usr/bin/sandbox-exec', ['-f', profile, command, ...args], { cwd, env, stdio: ['ignore', 'pipe', 'pipe'], detached: true });
    groups.add(child.pid);
    child.stdout.pipe(out);
    child.stderr.pipe(err);
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      stopGroup(child.pid, 'SIGTERM');
      setTimeout(() => stopGroup(child.pid, 'SIGKILL'), 5000);
    }, timeoutMs);
    child.on('close', (code) => {
      clearTimeout(timer);
      stopGroup(child.pid, 'SIGKILL');
      groups.delete(child.pid);
      out.end(() => resolve({ code, timedOut }));
    });
  });
}

// The lines Sikemux's native Claude driver appends to Claude's system prompt when it offers sikemux-tools.
export function sikemuxSystemLines(at) {
  const constant = (path, name) => readRepo(path, at).match(new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`))[1].replace(/\\(.)/g, '$1');
  return [
    constant('crates/sikemux-core/src/server/gestures.rs', 'PERSONALITY'),
    constant('crates/sikemux-core/src/server/chat/native/claude/mod.rs', 'SHOWING_PAGES'),
  ].join('\n\n');
}

export async function runClaude({ model, search, dir, sandbox, servers, prompt, maxTurns, timeoutMs, at }) {
  const config = join(dir, 'agent', 'mcp.json');
  writeFileSync(config, JSON.stringify(servers, null, 1));
  const args = [
    '-p',
    prompt,
    '--model',
    model,
    '--output-format',
    'stream-json',
    '--verbose',
    `--mcp-config=${config}`,
    '--strict-mcp-config',
    '--permission-mode',
    'bypassPermissions',
    '--max-turns',
    String(maxTurns),
    '--no-session-persistence',
    '--append-system-prompt',
    sikemuxSystemLines(at),
  ];
  const env = {
    ...sandbox.env,
    CLAUDE_CODE_OAUTH_TOKEN: claudeToken(),
    ENABLE_TOOL_SEARCH: search ? 'true' : 'false',
    ENABLE_CLAUDEAI_MCP_SERVERS: 'false',
    DISABLE_AUTOUPDATER: '1',
    CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY: '1',
  };
  const exit = await run({ command: join(process.env.HOME, '.local/bin/claude'), args, cwd: sandbox.repo, env, dir, timeoutMs });
  return { exit, ...readClaude(join(dir, 'transcript.jsonl')) };
}

export function canonical(name) {
  const server = name.match(SERVER_PREFIX);
  if (!server) return name;
  return server[1] === 'sikemux-tools' ? server[2] : `${server[1]}.${server[2]}`;
}

export function readClaude(path) {
  const calls = [];
  const byId = new Map();
  let reply = '';
  let result = null;
  let init = null;
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    if (!line.trim()) continue;
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      continue;
    }
    if (event.type === 'system' && event.subtype === 'init') init = event;
    if (event.type === 'assistant') {
      for (const block of event.message?.content ?? []) {
        if (block.type === 'tool_use') {
          const call = {
            id: block.id,
            tool: canonical(block.name),
            args: block.input ?? {},
            batch: event.message.id,
            sub: Boolean(event.parent_tool_use_id),
          };
          calls.push(call);
          byId.set(block.id, call);
        }
        if (block.type === 'text' && !event.parent_tool_use_id) reply = block.text;
      }
    }
    if (event.type === 'user') {
      for (const block of event.message?.content ?? []) {
        if (block.type !== 'tool_result') continue;
        const call = byId.get(block.tool_use_id);
        if (!call) continue;
        call.error = Boolean(block.is_error);
        call.result = resultText(block.content).slice(0, 1500);
      }
    }
    if (event.type === 'result') result = event;
  }
  const usage = result?.usage ?? {};
  return {
    calls,
    reply: result?.result ?? reply,
    failed: result ? result.is_error && result.subtype !== 'error_max_turns' : true,
    stop: result?.subtype ?? 'no result',
    turns: result?.num_turns ?? 0,
    cost: result?.total_cost_usd ?? 0,
    tokens: {
      input: (usage.input_tokens ?? 0) + (usage.cache_read_input_tokens ?? 0) + (usage.cache_creation_input_tokens ?? 0),
      cached: usage.cache_read_input_tokens ?? 0,
      output: usage.output_tokens ?? 0,
    },
    toolCount: init?.tools?.length ?? 0,
  };
}

function resultText(content) {
  if (typeof content === 'string') return content;
  return (content ?? []).map((block) => (block.type === 'text' ? block.text : `[${block.type}]`)).join('\n');
}
