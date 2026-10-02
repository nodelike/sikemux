// uniffi-bindgen-react-native links the Rust client into the module's shared
// library with every Rust symbol exported, so the linker keeps all of it: 27 MB
// of a library that is 11 MB on its own. This exports only what Android looks
// up and lets the linker drop code nothing reaches. The CMake file it edits is
// regenerated on every native build, so this runs after each one.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const cmake = resolve(here, '../android/CMakeLists.txt');
const marker = '# Linked small by scripts/shrink-android-link.mjs';
const text = readFileSync(cmake, 'utf8');
if (!text.includes(marker)) {
  writeFileSync(
    cmake,
    `${text}\n${marker}\ntarget_link_options(sikemux-client PRIVATE\n  "-Wl,--gc-sections"\n  "-Wl,--version-script=\${CMAKE_SOURCE_DIR}/../scripts/android-exports.map"\n)\n`,
  );
}
