import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ios = fileURLToPath(new URL('../../ios/', import.meta.url));
const vector = fileURLToPath(new URL('../../../server/protocol/vectors/push.json', import.meta.url));

export const swiftAvailable = process.platform === 'darwin' && spawnSync('xcrun', ['--find', 'swiftc']).status === 0;

/** Builds the extension's decryptor with mobile/ios/Tests/NotifyVector/main.swift and runs it on the core's vector. */
export function openVectorInSwift() {
  const dir = mkdtempSync(join(tmpdir(), 'sikemux-notify-'));
  try {
    const notify = readdirSync(join(ios, 'Shared', 'Notify')).map((name) => join(ios, 'Shared', 'Notify', name));
    const shared = [...notify, join(ios, 'Shared', 'Keychain.swift')];
    const binary = join(dir, 'check');
    execFileSync('xcrun', ['swiftc', '-o', binary, ...shared, join(ios, 'Tests', 'NotifyVector', 'main.swift')], { stdio: 'pipe' });
    return execFileSync(binary, [vector], { encoding: 'utf8' }).trim();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
