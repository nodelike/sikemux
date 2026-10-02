const { withAndroidManifest, withAppBuildGradle } = require('expo/config-plugins');

// The phone's APK is downloaded from a release page, so what counts is its size, not
// how fast Android maps it. Two things kept it large:
//  - code was stored uncompressed so Android could map it straight out of the APK;
//  - expo-camera bundles ML Kit's barcode model for scanning the pairing code, while
//    Google Play services already carries the same scanner on almost every phone.
const MARKER = '// Sized by plugins/small-apk.js';
const UNBUNDLED_SCANNER = 'com.google.android.gms:play-services-mlkit-barcode-scanning:18.3.1';

module.exports = (config) => {
  config = withAppBuildGradle(config, (config) => {
    if (!config.modResults.contents.includes(MARKER)) {
      config.modResults.contents += `
${MARKER}
android {
    packagingOptions {
        dex {
            useLegacyPackaging true
        }
    }
}
configurations.all {
    resolutionStrategy.dependencySubstitution {
        substitute module('com.google.mlkit:barcode-scanning') using module('${UNBUNDLED_SCANNER}')
    }
}
`;
    }
    return config;
  });
  return withAndroidManifest(config, (config) => {
    const application = config.modResults.manifest.application?.[0];
    if (!application) return config;
    application['meta-data'] = application['meta-data'] ?? [];
    const name = 'com.google.mlkit.vision.DEPENDENCIES';
    if (!application['meta-data'].some((entry) => entry.$['android:name'] === name)) {
      // Asks Play services to fetch the scanner when the app installs rather than on the first
      // scan, keeping the code-scanner UI expo-camera already asks for.
      application['meta-data'].push({
        $: { 'android:name': name, 'android:value': 'barcode_ui,barcode', 'tools:replace': 'android:value' },
      });
    }
    return config;
  });
};
