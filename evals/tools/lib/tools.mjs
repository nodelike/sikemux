import { createServer } from 'node:net';
import { spawn } from 'node:child_process';
import { join } from 'node:path';
import { BASE_PATH, sandboxed } from './sandbox.mjs';

// The tool server runs outside the agent's sandbox, so nothing the agent runs can start a Sikemux binary.
export async function startTools({ host, endpoint, sandbox, profile }) {
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
    socket.pipe(tools.stdin);
    tools.stdout.pipe(socket);
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
