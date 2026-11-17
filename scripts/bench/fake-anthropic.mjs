#!/usr/bin/env node
// A stand-in for Anthropic's Messages API on localhost, so agent CLIs run for real
// without spending tokens. Every turn is answered with one short line of text.
//
//   node fake-anthropic.mjs [port] [sessions]
//
// Given a session count, "bench session 1" in an app whose agents have a CreateWorkspace
// tool (Conductor) uses it to open sessions 2 to that count, one call per turn.
import { randomUUID } from "node:crypto";
import http from "node:http";

const port = Number(process.argv[2] ?? 0);
const sessionsToOpen = Number(process.argv[3] ?? 0);
const reply = "Ready.";
let turns = 0;
const sessions = new Set();

function readBody(request) {
  return new Promise((resolve) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
  });
}

const text = (content) =>
  typeof content === "string" ? content : (content ?? []).map((block) => block.text ?? "").join("");

function nextWorkspace(request) {
  const tool = request.tools?.find((candidate) => /CreateWorkspace$/.test(candidate.name ?? ""));
  const first = request.messages?.find((entry) => entry.role === "user");
  if (!sessionsToOpen || !tool || !/bench session 1:/.test(text(first?.content))) return null;
  const opened = (request.messages ?? [])
    .flatMap((entry) => (Array.isArray(entry.content) ? entry.content : []))
    .filter((block) => block.type === "tool_result" && !block.is_error).length;
  const session = opened + 2;
  if (session > sessionsToOpen) return null;
  return {
    type: "tool_use",
    id: `toolu_bench_${randomUUID().replaceAll("-", "")}`,
    name: tool.name,
    input: {
      workspaceId: randomUUID(),
      name: `bench-${session}`,
      firstSession: { sessionId: randomUUID(), messageId: randomUUID(), agent: "claude", message: `bench session ${session}: reply with one word.` },
    },
  };
}

function message(model, block) {
  return {
    id: `msg_bench_${++turns}`,
    type: "message",
    role: "assistant",
    model,
    content: [block],
    stop_reason: block.type === "tool_use" ? "tool_use" : "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 2 },
  };
}

function stream(response, model, block) {
  response.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive",
  });
  const send = (type, data) => response.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
  const whole = message(model, block);
  send("message_start", { message: { ...whole, content: [], stop_reason: null, usage: { input_tokens: 10, output_tokens: 0 } } });
  if (block.type === "tool_use") {
    send("content_block_start", { index: 0, content_block: { ...block, input: {} } });
    send("content_block_delta", { index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify(block.input) } });
  } else {
    send("content_block_start", { index: 0, content_block: { type: "text", text: "" } });
    send("content_block_delta", { index: 0, delta: { type: "text_delta", text: block.text } });
  }
  send("content_block_stop", { index: 0 });
  send("message_delta", { delta: { stop_reason: whole.stop_reason, stop_sequence: null }, usage: { output_tokens: 2 } });
  send("message_stop", {});
  response.end();
}

const server = http.createServer(async (request, response) => {
  const path = new URL(request.url, "http://localhost").pathname;
  if (path === "/bench/stats") {
    response.writeHead(200, { "content-type": "application/json" });
    return response.end(JSON.stringify({ turns, sessions: sessions.size }));
  }
  const body = request.method === "POST" ? await readBody(request) : "";
  const json = (status, value) => {
    response.writeHead(status, { "content-type": "application/json" });
    response.end(JSON.stringify(value));
  };
  if (path.endsWith("/messages/count_tokens")) return json(200, { input_tokens: 10 });
  if (path.endsWith("/messages")) {
    let parsed = {};
    try {
      parsed = JSON.parse(body);
    } catch {}
    for (const match of body.matchAll(/bench session (\d+)/g)) sessions.add(match[1]);
    const model = parsed.model ?? "claude-bench";
    const block = nextWorkspace(parsed) ?? { type: "text", text: reply };
    if (parsed.stream) return stream(response, model, block);
    return json(200, message(model, block));
  }
  if (path.endsWith("/models")) {
    return json(200, { data: [{ type: "model", id: "claude-bench", display_name: "Bench" }], has_more: false });
  }
  json(404, { type: "error", error: { type: "not_found_error", message: path } });
});

server.listen(port, "127.0.0.1", () => console.log(`http://127.0.0.1:${server.address().port}`));
