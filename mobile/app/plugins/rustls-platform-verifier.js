const fs = require('node:fs');
const path = require('node:path');
const { withAppBuildGradle, withDangerousMod, withProjectBuildGradle } = require('expo/config-plugins');

const MAVEN = 'https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/';
const LOCK = path.resolve(__dirname, '../../../src-tauri/Cargo.lock');

// The Kotlin half of iroh's certificate check must match the Rust crate's version exactly.
function verifierVersion() {
  const lock = fs.readFileSync(LOCK, 'utf8');
  const found = lock.match(/name = "rustls-platform-verifier-android"\nversion = "([^"]+)"/);
  if (!found) throw new Error(`rustls-platform-verifier-android is not in ${LOCK}`);
  return found[1];
}

// The verifier's network security config forbids plain HTTP, which a debug build needs to reach
// Metro. A debug resource of the same name replaces it; release builds keep the verifier's.
const DEBUG_NETWORK_CONFIG = `<?xml version="1.0" encoding="utf-8"?>
<network-security-config>
  <base-config cleartextTrafficPermitted="true" />
</network-security-config>
`;

module.exports = (config) => {
  config = withDangerousMod(config, [
    'android',
    (config) => {
      const dir = path.join(config.modRequest.platformProjectRoot, 'app/src/debug/res/xml');
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(path.join(dir, 'network_security_config.xml'), DEBUG_NETWORK_CONFIG);
      return config;
    },
  ]);
  config = withProjectBuildGradle(config, (config) => {
    if (!config.modResults.contents.includes(MAVEN)) {
      config.modResults.contents = config.modResults.contents.replace(
        /allprojects\s*\{\s*repositories\s*\{/,
        (opening) => `${opening}\n    maven { url "${MAVEN}" }`,
      );
    }
    return config;
  });
  return withAppBuildGradle(config, (config) => {
    const dependency = `implementation "org.rustls:rustls-platform-verifier:${verifierVersion()}"`;
    if (!config.modResults.contents.includes('org.rustls:rustls-platform-verifier')) {
      config.modResults.contents = config.modResults.contents.replace(/dependencies\s*\{/, (opening) => `${opening}\n    ${dependency}`);
    }
    return config;
  });
};
