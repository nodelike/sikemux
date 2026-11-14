// The `sikemux` command agents get on the cli surface. `sikemux tool` lists, explains and calls the same tools
// the MCP surface offers, by talking MCP to the same tool server, so only how the agent reaches them differs.
import { connect } from 'node:net';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const USAGE = `usage:
  sikemux tool                     list the tools, one line each
  sikemux tool help NAME           what NAME does and the arguments it takes
  sikemux tool NAME [JSON]         call NAME; JSON is its arguments, or - to read them from stdin
  sikemux tool instructions        how Sikemux expects agents to work`;

const [command, ...rest] = process.argv.slice(2);
if (command !== 'tool') {
  process.stderr.write(command === undefined || command === '--help' || command === 'help' ? `${USAGE}\n` : 'sikemux: only `sikemux tool` works in this shell\n');
  process.exit(command === undefined || command === '--help' || command === 'help' ? 0 : 1);
}

const server = await open(Number(process.env.SIKEMUX_TOOLS_PORT));
const [first, second, ...extra] = rest;
try {
  if (!first || first === 'list') console.log(listing(await server.tools()));
  else if (first === 'help' || first === '--help' || first === '-h') {
    if (!second) console.log(USAGE);
    else console.log(help(await server.tools(), second));
  } else if (first === 'instructions') console.log(`Sikemux's tools are subcommands of \`sikemux tool\`; the sikemux skill says how to call them.\n\n${server.instructions}`);
  else if (second === '--help' || second === '-h') console.log(help(await server.tools(), first));
  else {
    if (extra.length) fail(`sikemux tool ${first} takes one JSON argument; quote it, such as '{"url": "…"}'`);
    process.exitCode = await call(server, first, second);
  }
} finally {
  server.close();
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  server.close();
  process.exit(2);
}

async function call(server, name, given) {
  let args = {};
  if (given !== undefined) {
    const text = given === '-' ? readFileSync(0, 'utf8') : given;
    try {
      args = JSON.parse(text);
    } catch {
      fail(`the arguments are not JSON: ${text.slice(0, 200)}`);
    }
  }
  const reply = await server.request('tools/call', { name, arguments: args });
  if (reply.error) {
    console.log(reply.error.message);
    return 1;
  }
  const parts = (reply.result.content ?? []).map((block, index) => {
    if (block.type === 'text') return block.text;
    if (block.type === 'image') {
      const extension = (block.mimeType ?? 'image/png').split('/')[1].replace('jpeg', 'jpg');
      const path = join(process.env.TMPDIR ?? '/tmp', `sikemux-${name}-${Date.now()}-${index}.${extension}`);
      writeFileSync(path, Buffer.from(block.data, 'base64'));
      return `image saved to ${path}; Read it to see it`;
    }
    return `[${block.type}]`;
  });
  console.log(parts.join('\n'));
  return reply.result.isError ? 1 : 0;
}

function listing(tools) {
  const width = Math.max(...tools.map((tool) => tool.name.length)) + 2;
  return [...tools.map((tool) => `${tool.name.padEnd(width)}${firstSentence(tool.description)}`), '', '`sikemux tool help NAME` explains one.'].join('\n');
}

function firstSentence(text = '') {
  const line = text.split('\n')[0];
  const end = line.search(/\.(\s|$)/);
  return end < 0 ? line : line.slice(0, end + 1);
}

function help(tools, name) {
  const tool = tools.find((one) => one.name === name);
  if (!tool) return `no tool called ${name}; \`sikemux tool\` lists them`;
  const schema = tool.inputSchema ?? {};
  const fields = describeFields(schema, '  ');
  return [
    `${tool.name}: ${tool.description}`,
    '',
    fields.length ? `arguments (JSON):\n${fields.join('\n')}` : 'takes no arguments',
    '',
    `call: sikemux tool ${tool.name}${fields.length ? ` '${example(schema)}'` : ''}`,
  ].join('\n');
}

function describeFields(schema, indent) {
  const required = new Set(schema.required ?? []);
  return Object.entries(schema.properties ?? {}).flatMap(([field, spec]) => {
    const line = `${indent}${field}${required.has(field) ? '' : '?'}: ${typeOf(spec)}${spec.description ? `  ${spec.description}` : ''}`;
    const inner = spec.type === 'object' ? spec : spec.type === 'array' && spec.items?.type === 'object' ? spec.items : null;
    return [line, ...(inner ? describeFields(inner, `${indent}  `) : [])];
  });
}

function typeOf(spec) {
  if (spec.enum) return spec.enum.map((value) => JSON.stringify(value)).join(' | ');
  if (spec.oneOf || spec.anyOf) return (spec.oneOf ?? spec.anyOf).map(typeOf).join(' | ');
  if (spec.type === 'array') return `${spec.items ? typeOf(spec.items) : 'any'}[]`;
  if (Array.isArray(spec.type)) return spec.type.join(' | ');
  return spec.type ?? 'any';
}

function example(schema) {
  const required = schema.required?.length ? schema.required : Object.keys(schema.properties ?? {}).slice(0, 1);
  const value = (spec) => (spec?.enum ? spec.enum[0] : { string: '…', number: 0, integer: 0, boolean: true, array: [], object: {} }[[spec?.type].flat()[0]] ?? '…');
  return JSON.stringify(Object.fromEntries(required.map((field) => [field, value(schema.properties?.[field])])));
}

async function open(port) {
  const socket = connect(port, '127.0.0.1');
  await new Promise((resolve, reject) => {
    socket.once('connect', resolve);
    socket.once('error', () => reject(new Error('sikemux: Sikemux is not running')));
  }).catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exit(1);
  });
  const waiting = new Map();
  let buffer = '';
  let next = 1;
  socket.on('data', (chunk) => {
    buffer += chunk;
    let newline;
    while ((newline = buffer.indexOf('\n')) >= 0) {
      const message = JSON.parse(buffer.slice(0, newline));
      buffer = buffer.slice(newline + 1);
      waiting.get(message.id)?.(message);
      waiting.delete(message.id);
    }
  });
  socket.on('close', () => {
    for (const resolve of waiting.values()) resolve({ error: { message: 'Sikemux closed the connection' } });
  });
  const request = (method, params) =>
    new Promise((resolve) => {
      const id = next++;
      waiting.set(id, resolve);
      socket.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
    });
  const hello = await request('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'sikemux-cli', version: '1' } });
  socket.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);
  let listed = null;
  return {
    instructions: hello.result?.instructions ?? '',
    request,
    async tools() {
      listed ??= (await request('tools/list', {})).result?.tools ?? [];
      return listed;
    },
    close: () => socket.destroy(),
  };
}
