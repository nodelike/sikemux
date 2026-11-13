import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { run, claudeToken } from './runners.mjs';
import { CLAUDE } from './sandbox.mjs';
import { brief } from './grade.mjs';

export const JUDGE_MODEL = 'claude-sonnet-5-5';
const ROLE =
  'You grade one run of a coding agent in a tool-use eval. Read the rubric, the task the agent was given, every tool call it made with a short piece of each result, and its final reply, then give a verdict.';

export async function judge({ testCase, outcome, dir, sandbox }) {
  const folder = join(dir, 'judge');
  mkdirSync(folder, { recursive: true });
  const steps = outcome.calls
    .map((call, index) => `${index + 1}. ${brief(call)}${call.error ? '  -> ERROR' : ''}${call.result ? `\n   -> ${call.result.slice(0, 300).replace(/\n/g, ' ')}` : ''}`)
    .join('\n');
  const prompt = [
    `Rubric:\n${testCase.judge}`,
    `Task given to the agent:\n${testCase.prompt}`,
    `Tool calls:\n${steps || '(none)'}`,
    `Final reply:\n${outcome.reply || '(none)'}`,
    'Answer with only a JSON object and nothing else: {"pass": true or false, "why": "one sentence"}.',
  ].join('\n\n');
  await run({
    command: CLAUDE,
    args: ['-p', prompt, '--system-prompt', ROLE, '--model', JUDGE_MODEL, '--output-format', 'json', '--tools', '', '--strict-mcp-config', '--no-session-persistence', '--max-turns', '1'],
    cwd: sandbox.repo,
    env: { ...sandbox.env, CLAUDE_CODE_OAUTH_TOKEN: claudeToken(), ENABLE_CLAUDEAI_MCP_SERVERS: 'false', DISABLE_AUTOUPDATER: '1' },
    dir: folder,
    profile: join(dir, 'sandbox.sb'),
    timeoutMs: 180_000,
  });
  return readVerdict(folder) ?? { pass: false, why: 'the judge gave no verdict', cost: 0 };
}

export function readVerdict(folder) {
  try {
    const answer = JSON.parse(readFileSync(join(folder, 'transcript.jsonl'), 'utf8'));
    const verdict = JSON.parse(answer.result.match(/\{[^{}]*"pass"[^{}]*\}/)[0]);
    return { pass: Boolean(verdict.pass), why: String(verdict.why ?? ''), cost: answer.total_cost_usd ?? 0 };
  } catch {
    return null;
  }
}
