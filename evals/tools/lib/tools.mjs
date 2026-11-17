import { createServer } from 'node:net';
import { spawn } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { join } from 'node:path';
import { BASE_PATH, sandboxed } from './sandbox.mjs';

// The tool server runs outside the agent's sandbox, so nothing the agent runs can start a Sikemux binary.
// Every tools/call that reaches it is written to `log`, whichever surface the agent called it from.
export async function startTools({ host, endpoint, sandbox, profile, log }) {
  const servers = new Set();
  const server = createServer((socket) => {
    const tools = spawn(...sandboxed(profile, host, ['--tools-mcp']), {
      env: {
        HOME: sandbox.home,
        USER: process.env.USER,
        PATH: BASE_PATH,
        SIKEMUX_TOOLS_AGENT_ID: 'agent-eval-self',
        SIKEMUX_CLI_ENDPOINT: endpoint,
        SIKEMUX_PROJECT: sandbox.repo,
      },
      stdio: ['pipe', 'pipe', 'ignore'],
    });
    servers.add(tools);
    const asked = new Map();
    socket.pipe(tools.stdin);
    tools.stdout.pipe(socket);
    eachMessage(socket, (message) => {
      if (message.method === 'tools/call' && message.id !== undefined) {
        asked.set(message.id, { at: Date.now(), tool: message.params?.name, args: message.params?.arguments ?? {} });
      }
    });
    eachMessage(tools.stdout, (message) => {
      const call = message.id !== undefined && asked.get(message.id);
      if (!call) return;
      asked.delete(message.id);
      const result = message.result ?? {};
      const text = (result.content ?? []).map((block) => (block.type === 'text' ? block.text : `[${block.type}]`)).join('\n');
      appendFileSync(log, `${JSON.stringify({ ...call, error: Boolean(message.error || result.isError), result: (text || message.error?.message || '').slice(0, 1500) })}\n`);
    });
    socket.on('error', () => {});
    tools.stdin.on('error', () => {});
    socket.on('close', () => tools.kill('SIGKILL'));
    tools.on('exit', () => {
      servers.delete(tools);
      socket.destroy();
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return {
    port: server.address().port,
    config: { command: process.execPath, args: [join(import.meta.dirname, 'relay.mjs'), String(server.address().port)] },
    close() {
      for (const tools of servers) tools.kill('SIGKILL');
      server.close();
    },
  };
}

function eachMessage(stream, onMessage) {
  let buffer = '';
  stream.on('data', (chunk) => {
    buffer += chunk;
    let newline;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      try {
        onMessage(JSON.parse(line));
      } catch {}
    }
  });
}
