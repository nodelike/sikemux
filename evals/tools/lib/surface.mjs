import { readFileSync, readdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

export const REPO = `${join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')}/`;

export const SIGNED_IN_PLUGINS = ['aws', 'jira', 'rundeck', 'signoz', 'vercel'];

export function readRepo(path, at) {
  if (!at) return readFileSync(join(REPO, path), 'utf8');
  return execFileSync('git', ['show', `${at}:${path}`], { cwd: REPO, encoding: 'utf8', maxBuffer: 64 << 20 });
}

export function loadSurface(plugins = SIGNED_IN_PLUGINS, at = null) {
  const manifest = JSON.parse(readRepo('browser/tools.json', at));
  const guide = readRepo(`browser/${manifest.guide.file}`, at);
  const byMethod = new Map(manifest.tools.map((tool) => [tool.method, tool.name]));
  const pluginTools = plugins.flatMap((folder) => {
    const plugin = JSON.parse(readRepo(`plugins/${folder}/manifest.json`, at));
    return (plugin.tools ?? []).map((tool) => ({
      plugin: plugin.id,
      name: tool.name,
      method: tool.method,
      description: tool.description,
      properties: tool.properties ?? {},
      required: tool.required ?? [],
    }));
  });
  return { manifest, guide, byMethod, pluginTools };
}

export function allPlugins() {
  return readdirSync(join(REPO, 'plugins'), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
}
