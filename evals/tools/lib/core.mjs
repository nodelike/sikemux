import { createServer } from 'node:net';
import { createHmac, randomBytes } from 'node:crypto';
import { appendFileSync, writeFileSync } from 'node:fs';
import { answerer } from './answers.mjs';

export async function startCore({ surface, fixture, project, endpointFile, logFile, previewProfile }) {
  const token = randomBytes(16).toString('hex');
  const answer = answerer(fixture, project, previewProfile);
  const held = new Set();
  const log = (entry) => appendFileSync(logFile, `${JSON.stringify({ at: Date.now(), ...entry })}\n`);

  const handle = (method, params) => {
    switch (method) {
      case 'tools.manifest':
        return { value: { manifest: surface.manifest, guide: surface.guide } };
      case 'plugins.tools':
        return { value: surface.pluginTools };
      case 'plugins.changed':
        return params?.seen == null ? { value: { version: 0 } } : null;
      case 'sim.offered':
        return { value: { offered: true } };
      case 'sim.cancel':
        return { value: {} };
      case 'plugins.call':
        return call(params.tool, params.arguments ?? {});
      default: {
        const tool = surface.byMethod.get(method);
        if (!tool) return { error: `Unknown harness method ${method}` };
        return call(tool, params ?? {});
      }
    }
  };

  const call = (tool, args) => {
    const result = answer(tool, args);
    log({ tool, args, ...(result.error ? { error: result.error } : { reply: result.value }) });
    return result;
  };

  const server = createServer((socket) => {
    let buffer = '';
    let stage = 'hello';
    socket.on('error', () => {});
    socket.on('data', (chunk) => {
      buffer += chunk;
      let newline;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, newline);
        buffer = buffer.slice(newline + 1);
        const message = JSON.parse(line);
        if (stage === 'hello') {
          const port = server.address().port;
          const proof = createHmac('sha256', token).update(`sikemux-cli-server\n${port}\n${message.nonce}`).digest('hex');
          socket.write(`${JSON.stringify({ status: 'hello', proof })}\n`);
          stage = 'request';
          continue;
        }
        if (!message?.request?.method) {
          socket.end(`${JSON.stringify({ status: 'error', message: 'not a harness request' })}\n`);
          continue;
        }
        const { method, params } = message.request;
        const result = handle(method, params);
        if (!result) {
          held.add(socket);
          continue;
        }
        const frame = result.error ? { status: 'error', message: result.error } : { status: 'result', value: result.value };
        socket.end(`${JSON.stringify(frame)}\n`);
      }
    });
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  writeFileSync(endpointFile, JSON.stringify({ protocol: 2, pid: process.pid, port: server.address().port, token, version: 'eval' }));
  return {
    close() {
      for (const socket of held) socket.destroy();
      server.close();
    },
  };
}
