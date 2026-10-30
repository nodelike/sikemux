const { existsSync, readdirSync, readFileSync } = require('node:fs');
const { join } = require('node:path');

/** The packages in a Cargo.lock: name, version, source, checksum and what each depends on. */
function readLock(text) {
  return text
    .split(/\n(?=\[\[package\]\]\n)/)
    .filter((block) => block.startsWith('[[package]]'))
    .map((block) => {
      const field = (name) => new RegExp(`^${name} = "(.*)"$`, 'm').exec(block)?.[1];
      const list = /^dependencies = \[\n([\s\S]*?)\n\]$/m.exec(block)?.[1] ?? '';
      return {
        name: field('name'),
        version: field('version'),
        source: field('source'),
        checksum: field('checksum'),
        dependencies: [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]),
      };
    });
}

/** The names a crate's Cargo.toml lists under [dev-dependencies], which never reach the phone. */
function devDependencies(manifest) {
  const section = /^\[dev-dependencies\]\n([\s\S]*?)(?=^\[|$(?![\s\S]))/m.exec(manifest)?.[1] ?? '';
  return new Set([...section.matchAll(/^([A-Za-z0-9_-]+)\s*=/gm)].map((match) => match[1]));
}

/**
 * Every package `root` builds with, following Cargo.lock from it. Lock entries name a dependency as
 * "name", or "name version" when several versions are locked. A workspace crate's dev-dependencies
 * are left out through `devOf`, which lists them by crate name.
 */
function linkedPackages(packages, root, devOf = () => new Set()) {
  const byName = new Map();
  for (const pkg of packages) byName.set(pkg.name, [...(byName.get(pkg.name) ?? []), pkg]);
  const find = (spec) => {
    const [name, version] = spec.split(' ');
    const candidates = byName.get(name) ?? [];
    const found = version ? candidates.find((pkg) => pkg.version === version) : candidates[0];
    if (!found) throw new Error(`Cargo.lock names ${spec}, which it does not lock`);
    return found;
  };
  const seen = new Set();
  const queue = [find(root)];
  while (queue.length > 0) {
    const pkg = queue.shift();
    if (seen.has(pkg)) continue;
    seen.add(pkg);
    const dev = pkg.source ? new Set() : devOf(pkg.name);
    for (const spec of pkg.dependencies) if (!dev.has(spec.split(' ')[0])) queue.push(find(spec));
  }
  return [...seen];
}

/**
 * The workspace's own settings that shape every build: [workspace…], [profile…], [patch…] and [replace…], not
 * the desktop app's package. Settings for one crate, a profile override or a patch, are kept only when `links`
 * names it, so a setting for a crate the phone never builds does not change its runtime.
 */
function workspaceSettings(manifest, links = () => true) {
  const forCrate = (header) => /^\[profile\.[^.\]]+\.package\.(?:"([^"]+)"|([^\]]+))\]$/.exec(header);
  const entryName = (line) => /^"?([A-Za-z0-9_-]+)/.exec(line)?.[1];
  return manifest
    .split(/\n(?=\[)/)
    .map((section) => section.split('\n').filter((line) => line.trim() !== '' && !line.trim().startsWith('#')))
    .filter(([header]) => /^\[(workspace|profile|patch|replace)\b/.test(header))
    .filter(([header]) => {
      const crate = forCrate(header);
      return !crate || crate[1] === '*' || links(crate[1] ?? crate[2]);
    })
    .map(([header, ...lines]) => {
      if (header === '[workspace.package]') return [header, ...lines.filter((line) => !line.startsWith('version ='))];
      if (/^\[(patch|replace)\b/.test(header)) return [header, ...lines.filter((line) => links(entryName(line)))];
      return [header, ...lines];
    })
    .filter((lines) => lines.length > 1)
    .map((lines) => `${lines.join('\n')}\n`)
    .join('\n');
}

/** Where each workspace crate lives under `crates`, by its package name. */
function crateDirs(crates) {
  const dirs = new Map();
  for (const dir of readdirSync(crates)) {
    const path = join(crates, dir, 'Cargo.toml');
    if (!existsSync(path)) continue;
    const manifest = readFileSync(path, 'utf8');
    const name = /^name = "(.*)"$/m.exec(manifest)?.[1];
    if (name) dirs.set(name, { dir, manifest });
  }
  return dirs;
}

/**
 * What the phone's Rust client is built from: the workspace crates it links with their sources, the
 * locked version of every other crate it links, and the workspace's build settings. The desktop app's
 * own version and dependencies are left out, so a desktop release does not change the phone's runtime.
 */
function rustClientSources(workspace, root = 'sikemux-mobile') {
  const crates = crateDirs(join(workspace, 'crates'));
  const devOf = (name) => devDependencies(crates.get(name)?.manifest ?? '');
  const linked = linkedPackages(readLock(readFileSync(join(workspace, 'Cargo.lock'), 'utf8')), root, devOf);
  const local = linked
    .filter((pkg) => !pkg.source)
    .map((pkg) => {
      const crate = crates.get(pkg.name);
      if (!crate) throw new Error(`${pkg.name} is in the workspace but not under ${join(workspace, 'crates')}`);
      return crate.dir;
    });
  const names = new Set(linked.map((pkg) => pkg.name));
  const locked = linked
    .filter((pkg) => pkg.source)
    .map((pkg) => `${pkg.name} ${pkg.version} ${pkg.source} ${pkg.checksum ?? ''}`)
    .sort();
  return {
    crates: local.sort(),
    locked: locked.join('\n'),
    settings: workspaceSettings(readFileSync(join(workspace, 'Cargo.toml'), 'utf8'), (name) => names.has(name)),
  };
}

module.exports = { readLock, devDependencies, linkedPackages, workspaceSettings, rustClientSources };
