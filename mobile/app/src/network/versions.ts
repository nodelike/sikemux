import type { ChannelVersions } from '@sikemux/protocol';

/**
 * The installed app's version from its build number, which release builds set to the Play version code:
 * 0.5.0-nightly.3 is 50003, and 0.5.0 itself is 50099. Anything else is not a release build's.
 */
export function versionFromBuild(build: string | null): string | null {
  if (!build || !/^\d+$/.test(build)) return null;
  const code = Number(build);
  if (code < 100) return null;
  const major = Math.floor(code / 1_000_000);
  const minor = Math.floor(code / 10_000) % 100;
  const patch = Math.floor(code / 100) % 100;
  const nightly = code % 100;
  const base = `${major}.${minor}.${patch}`;
  return nightly === 99 ? base : `${base}-nightly.${nightly}`;
}

type Parsed = { core: [number, number, number]; pre: string[] };

function parse(version: string): Parsed | null {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(version);
  if (!match) return null;
  return { core: [Number(match[1]), Number(match[2]), Number(match[3])], pre: match[4] ? match[4].split('.') : [] };
}

function compareIdentifiers(a: string, b: string): number {
  const numeric = /^\d+$/;
  if (numeric.test(a) && numeric.test(b)) return Number(a) - Number(b);
  if (numeric.test(a)) return -1;
  if (numeric.test(b)) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Semantic version order: negative when `a` is older. */
export function compareVersions(a: string, b: string): number | null {
  const left = parse(a);
  const right = parse(b);
  if (!left || !right) return null;
  for (let i = 0; i < 3; i += 1) if (left.core[i] !== right.core[i]) return left.core[i] - right.core[i];
  if (left.pre.length === 0 || right.pre.length === 0) return right.pre.length - left.pre.length;
  for (let i = 0; i < Math.min(left.pre.length, right.pre.length); i += 1) {
    const order = compareIdentifiers(left.pre[i], right.pre[i]);
    if (order !== 0) return order;
  }
  return left.pre.length - right.pre.length;
}

/** The oldest version allowed for `installed`'s channel when `installed` is older; `0.0.0` allows every version. */
export function tooOld(installed: string, minimum: ChannelVersions): string | null {
  const oldest = installed.includes('-') ? minimum.nightly : minimum.stable;
  if (oldest === '0.0.0') return null;
  const order = compareVersions(installed, oldest);
  return order !== null && order < 0 ? oldest : null;
}
