#!/usr/bin/env node
// The built sidecar must start, list its tools, serve its guide, and relay a
// browser call to a stand-in for the app over the harness socket.

import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { createServer } from "node:net";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const STATE = {
  url: "https://example.com",
  title: "Sikemux Browser Smoke",
  elements: "[0] <button> ready",
  text: "ready",
  tabs: [],
};
const EXPECTED_TOOLS = [
  "browser_navigate",
  "browser_state",
  "browser_click",
  "browser_screenshot",
  "workspace_inspect",
  "guide",
];

function fakeSikemux(received) {
  const server = createServer((socket) => {
    let frame = "";
    socket.on("data", (chunk) => {
      frame += chunk;
      const newline = frame.indexOf("\n");
      if (newline < 0) return;
      const call = JSON.parse(frame.slice(0, newline));
      received.push(call);
      const value = call.request?.method === "plugins.tools" ? [] : STATE;
      socket.end(`${JSON.stringify({ status: "result", value })}\n`);
    });
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(server));
  });
}

function session(sidecar, environment) {
  const child = spawn(sidecar, [], {
    env: environment,
    stdio: ["pipe", "pipe", "inherit"],
  });
  const pending = new Map();
  let nextId = 0;
  createInterface({ input: child.stdout }).on("line", (line) => {
    const message = JSON.parse(line);
    pending.get(message.id)?.(message);
    pending.delete(message.id);
  });
  return {
    child,
    notify: (method) =>
      child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", method })}\n`),
    request(method, params) {
      const id = (nextId += 1);
      return new Promise((resolve, reject) => {
        pending.set(id, (message) =>
          message.error
            ? reject(new Error(`${method}: ${message.error.message}`))
            : resolve(message.result),
        );
        child.stdin.write(
          `${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`,
        );
        setTimeout(() => reject(new Error(`${method} timed out`)), 30_000);
      });
    },
  };
}

async function exercise(sidecar, environment, received) {
  const mcp = session(sidecar, environment);
  try {
    const start = await mcp.request("initialize", {
      protocolVersion: "2025-06-18",
      capabilities: {},
      clientInfo: { name: "sikemux-smoke", version: "1" },
    });
    if (start.serverInfo?.name !== "sikemux-tools")
      throw new Error(`unexpected server: ${JSON.stringify(start.serverInfo)}`);
    mcp.notify("notifications/initialized");

    const names = new Set(
      (await mcp.request("tools/list", {})).tools.map((tool) => tool.name),
    );
    for (const expected of EXPECTED_TOOLS) {
      if (!names.has(expected))
        throw new Error(`the sidecar does not expose ${expected}`);
    }

    const guide = await mcp.request("tools/call", {
      name: "guide",
      arguments: {},
    });
    if (
      guide.isError ||
      !guide.content[0].text.includes("Working inside Sikemux")
    )
      throw new Error("the sidecar does not carry its guide");

    const navigated = await mcp.request("tools/call", {
      name: "browser_navigate",
      arguments: { url: "https://example.com" },
    });
    if (
      navigated.isError ||
      JSON.stringify(JSON.parse(navigated.content[0].text)) !==
        JSON.stringify(STATE)
    )
      throw new Error(
        `the sidecar relayed the wrong answer: ${JSON.stringify(navigated.content)}`,
      );

    const call = received.find(
      (entry) => entry?.request?.method === "browser.navigate",
    );
    if (
      call?.request?.method !== "browser.navigate" ||
      call?.request?.agentId !== "agent-smoke" ||
      call?.token !== "smoke-token"
    )
      throw new Error(
        `the app did not receive the browser call: ${JSON.stringify(received)}`,
      );
  } finally {
    mcp.child.stdin.end();
    mcp.child.kill("SIGKILL");
  }
}

export async function smokeBrowserSidecar(sidecar) {
  const received = [];
  const server = await fakeSikemux(received);
  const directory = mkdtempSync(join(tmpdir(), "sikemux-smoke-"));
  try {
    const endpoint = join(directory, "endpoint.json");
    writeFileSync(
      endpoint,
      JSON.stringify({
        protocol: 1,
        pid: process.pid,
        port: server.address().port,
        token: "smoke-token",
        version: "smoke",
      }),
    );
    await exercise(
      sidecar,
      {
        ...process.env,
        SIKEMUX_CLI_ENDPOINT: endpoint,
        SIKEMUX_PROJECT: directory,
        SIKEMUX_TOOLS_AGENT_ID: "agent-smoke",
      },
      received,
    );
  } finally {
    server.close();
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1]?.endsWith("smoke-browser-sidecar.mjs")) {
  const sidecar = process.argv[2];
  if (!sidecar) {
    console.error("usage: smoke-browser-sidecar.mjs <sidecar>");
    process.exit(2);
  }
  try {
    await smokeBrowserSidecar(sidecar);
    console.log("✓ browser sidecar smoke passed");
  } catch (error) {
    console.error(`browser sidecar smoke failed: ${error.message}`);
    process.exit(1);
  }
}
