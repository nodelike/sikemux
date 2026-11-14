import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const SHELLS = ['Bash', 'BashOutput', 'Monitor'];
const HEREDOC = /sikemux\s+tool\b[^\n]*<<-?\s*(['"]?)(\w+)\1[^\n]*\n[\s\S]*?\n\s*\2[ \t]*(?=\n|$)/g;
const INLINE = /sikemux\s+tool(?:[ \t]+(?:'(?:[^']|'\\'')*'|"(?:\\.|[^"\\])*"|[^\s;&|()<>'"]+))*/g;
const PIPED_INTO = /\|\s*(?:head|tail|jq|cat|grep|sed|awk|wc|less)\b(?:'[^']*'|"[^"]*"|[^;&|\n'"])*/g;
const REDIRECT = /\d?>&\d|&>\s*\S+|\d?>\s*\/dev\/null/g;
const ONLY_SEPARATORS = /^[\s;&|()]*$/;
const BACKGROUND = /(?<![&>|])&(?![&>])/;

function readLines(path) {
  if (!existsSync(path)) return [];
  return readFileSync(path, 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line));
}

// The calls a case is graded on, the same on either surface: the agent's own calls (shell, files, other MCPs) with every
// call that reached Sikemux's tool server put where the agent made it. On mcp a tool call is the server's log entry in
// place of the transcript's; on cli the entries a shell command made follow it, and the command keeps only what is
// left once its `sikemux tool` invocations are cut out, or goes when nothing is.
export function servedCalls(surface, calls, dir) {
  const door = readLines(join(dir, 'calls.jsonl'));
  if (surface === 'mcp') {
    return calls.flatMap((call) => {
      if (!call.served) return [call];
      const index = door.findIndex((entry) => entry.tool === call.tool);
      if (index < 0) return [];
      const [entry] = door.splice(index, 1);
      return [{ ...call, args: entry.args, error: entry.error, result: entry.result }];
    });
  }
  const started = new Map(readLines(join(dir, 'hooks.jsonl')).map((hook) => [hook.id, hook.at]));
  const made = new Map();
  for (const entry of door) {
    const owner = calls.findLast((call) => SHELLS.includes(call.tool) && (started.get(call.id) ?? Infinity) <= entry.at);
    if (!owner) continue;
    made.set(owner, [...(made.get(owner) ?? []), entry]);
  }
  return calls.flatMap((call) => {
    const entries = made.get(call) ?? [];
    if (!SHELLS.includes(call.tool) || typeof call.args.command !== 'string') return [call];
    const command = call.args.command;
    const left = command.replace(HEREDOC, '').replace(INLINE, '');
    const invoked = left !== command;
    if (!invoked) return [call];
    const concurrent = Boolean(call.args.run_in_background) || BACKGROUND.test(command.replace(HEREDOC, ''));
    const served = entries.map((entry, index) => ({
      id: `${call.id}:${index}`,
      tool: entry.tool,
      args: entry.args,
      error: entry.error,
      result: entry.result,
      batch: call.batch,
      lane: call.id,
      concurrent,
      sub: call.sub,
    }));
    const plumbing = ONLY_SEPARATORS.test(left.replace(PIPED_INTO, '').replace(REDIRECT, ''));
    return plumbing ? served : [{ ...call, args: { ...call.args, command: left } }, ...served];
  });
}
