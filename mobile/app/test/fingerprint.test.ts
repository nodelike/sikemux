import { describe, expect, it } from 'vitest';

import fingerprint from '../fingerprint.config.js';
import { devDependencies, linkedPackages, readLock, rustClientSources, workspaceSettings } from '../scripts/rust-sources.js';

const LOCK = `version = 4

[[package]]
name = "app"
version = "0.5.0-nightly.6"
dependencies = [
 "tauri",
 "phone",
]

[[package]]
name = "phone"
version = "0.1.0"
dependencies = [
 "base64 0.22.1",
 "tempfile",
]

[[package]]
name = "base64"
version = "0.21.7"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "old"

[[package]]
name = "base64"
version = "0.22.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "new"

[[package]]
name = "tauri"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "t"
dependencies = [
 "base64 0.21.7",
]

[[package]]
name = "tempfile"
version = "3.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "f"
`;

describe('the Rust client in the fingerprint', () => {
  it('follows Cargo.lock from the phone crate, to the locked version each entry names', () => {
    const linked = linkedPackages(readLock(LOCK), 'phone', () => new Set(['tempfile']));
    expect(linked.map((pkg) => `${pkg.name} ${pkg.version}`).sort()).toEqual(['base64 0.22.1', 'phone 0.1.0']);
  });

  it("leaves out a workspace crate's dev-dependencies", () => {
    const manifest =
      '[package]\nname = "phone"\n\n[dependencies]\nserde = "1"\n\n[dev-dependencies]\ntempfile = "3"\nproptest = { version = "1" }\n\n[lints]\n';
    expect([...devDependencies(manifest)]).toEqual(['tempfile', 'proptest']);
  });

  it("keeps the workspace's build settings but not the desktop app's version or dependencies", () => {
    const manifest =
      '[package]\nname = "sikemux"\nversion = "0.5.0"\n\n[workspace]\nmembers = ["crates/*"]\n\n[dependencies]\ntauri = "2"\n\n[profile.release]\nlto = true\n';
    expect(workspaceSettings(manifest)).toBe('[workspace]\nmembers = ["crates/*"]\n\n[profile.release]\nlto = true\n');
  });

  it('counts the crates sikemux-mobile links in this workspace, and none the desktop app alone uses', () => {
    const sources = rustClientSources(new URL('../../../src-tauri', import.meta.url).pathname);
    expect(sources.crates).toEqual(['sikemux-client', 'sikemux-mobile', 'sikemux-wire']);
    expect(sources.locked).toMatch(/^iroh /m);
    expect(sources.locked).not.toMatch(/^(tauri|wry|tao) /m);
    expect(sources.settings).not.toMatch(/^version = /m);
  });

  it('hands those to the fingerprint as its Rust sources', () => {
    const rust = (fingerprint.extraSources ?? []).filter((source: { reasons: string[] }) => source.reasons.includes('rustClient'));
    const named = rust.map((source: { filePath?: string; id?: string }) => source.filePath ?? source.id);
    expect(named).toEqual(
      expect.arrayContaining([
        '../../rust-toolchain.toml',
        '../../src-tauri/crates/sikemux-client',
        '../../src-tauri/crates/sikemux-mobile',
        '../../src-tauri/crates/sikemux-wire',
        'rust/locked',
        'rust/workspace',
      ]),
    );
    expect(named).not.toContain('../../src-tauri/Cargo.toml');
    expect(named).not.toContain('../../src-tauri/Cargo.lock');
  });
});
