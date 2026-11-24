import { execFileSync, spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ios = fileURLToPath(new URL('../../ios/', import.meta.url));

export const swiftAvailable = process.platform === 'darwin' && spawnSync('xcrun', ['--find', 'swiftc']).status === 0;

/** Compiles mobile/ios/Sikemux/Core/RaceTimeout.swift with mobile/ios/Tests/ReconnectCheck/main.swift and runs it. */
export function runIosReconnectCheck() {
  const dir = mkdtempSync(join(tmpdir(), 'sikemux-ios-reconnect-'));
  try {
    const binary = join(dir, 'check');
    execFileSync(
      'xcrun',
      [
        'swiftc',
        '-parse-as-library',
        '-o',
        binary,
        join(ios, 'Sikemux', 'Core', 'RaceTimeout.swift'),
        join(ios, 'Tests', 'ReconnectCheck', 'main.swift'),
      ],
      { stdio: 'pipe' },
    );
    return execFileSync(binary, [], { encoding: 'utf8' }).trim();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
