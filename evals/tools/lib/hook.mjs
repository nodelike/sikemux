// Claude's PreToolUse hook: when each tool call started, so calls the tool server logged can be put among them.
import { appendFileSync, readFileSync } from 'node:fs';

const event = JSON.parse(readFileSync(0, 'utf8'));
appendFileSync(`${process.env.TOOL_EVAL_CASE}/hooks.jsonl`, `${JSON.stringify({ at: Date.now(), id: event.tool_use_id, tool: event.tool_name })}\n`);
