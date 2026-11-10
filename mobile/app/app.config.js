/**
 * Builds are "Sikemux Dev" unless APP_VARIANT=production. The two install side by side,
 * each with its own key and paired hosts.
 */
const release = require('../../server/protocol/src/release.json');

/** Google's OAuth web clients: production's in the `sikemux` Google Cloud project, dev's in `sikemux-dev`. */
const GOOGLE_WEB_CLIENT = {
  production: '225181228835-furfr1rb7o5uhghb4i1rn7vd0igh2g1i.apps.googleusercontent.com',
  dev: '479341813252-grbmpsl75qg37pflcqagq5kmejhmso7m.apps.googleusercontent.com',
};

/** Where accounts live: production's Clerk instance and api.sikemux.com, or Clerk's development instance and the Mac's own server. */
const ACCOUNTS = {
  production: release.accounts.production,
  dev: { clerkPublishableKey: release.accounts.dev.clerkPublishableKey },
};

/**
 * Play needs a number that grows with every upload: 0.5.0-nightly.3 is 50003, and 0.5.0 itself is 50099.
 * The iPhone app's build number is the same, so it can tell its own nightly from it.
 */
function versionCode(version, base) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-nightly\.(\d+))?$/.exec(version);
  if (!match) throw new Error(`${version} is not a version like 0.5.0 or 0.5.0-nightly.3`);
  const [major, minor, patch] = match.slice(1, 4).map(Number);
  const nightly = match[4] === undefined ? null : Number(match[4]);
  if (`${major}.${minor}.${patch}` !== base) throw new Error(`${version} is not a release of ${base}, the version in app.json`);
  if (minor > 99 || patch > 99 || nightly > 98) throw new Error(`${version} does not fit the version code scheme`);
  return major * 1_000_000 + minor * 10_000 + patch * 100 + (nightly ?? 99);
}

const CHANNELS = release.channels;

/** Production builds fetch over-the-air updates for their channel; the server only sends one signed with our key. */
function updates(channel) {
  if (!CHANNELS.includes(channel)) throw new Error(`${channel} is not a release channel: use nightly or stable`);
  return {
    enabled: true,
    url: 'https://updates.sikemux.com/manifest',
    checkAutomatically: 'ON_LOAD',
    fallbackToCacheTimeout: 0,
    codeSigningCertificate: './certs/updates-certificate.pem',
    codeSigningMetadata: { keyid: 'main', alg: 'rsa-v1_5-sha256' },
    requestHeaders: { 'expo-channel-name': channel },
  };
}

module.exports = ({ config }) => {
  if (process.env.APP_VARIANT === 'production') {
    const version = process.env.SIKEMUX_MOBILE_VERSION ?? config.version;
    const code = versionCode(version, config.version);
    return {
      ...config,
      extra: { ...config.extra, EXPO_PUBLIC_CLERK_GOOGLE_WEB_CLIENT_ID: GOOGLE_WEB_CLIENT.production, ...ACCOUNTS.production },
      android: { ...config.android, versionCode: code, googleServicesFile: './firebase/google-services.json' },
      runtimeVersion: { policy: 'fingerprint' },
      updates: updates(process.env.SIKEMUX_MOBILE_CHANNEL || 'nightly'),
    };
  }
  return {
    ...config,
    name: 'Sikemux Dev',
    updates: { enabled: false },
    extra: { ...config.extra, EXPO_PUBLIC_CLERK_GOOGLE_WEB_CLIENT_ID: GOOGLE_WEB_CLIENT.dev, ...ACCOUNTS.dev },
    scheme: 'sikemux-dev',
    android: {
      ...config.android,
      package: `${config.android.package}.dev`,
      googleServicesFile: './firebase/google-services.dev.json',
      adaptiveIcon: {
        ...config.android.adaptiveIcon,
        backgroundColor: '#140c2a',
        backgroundImage: './assets/images/android-icon-background-dev.png',
      },
    },
  };
};

module.exports.versionCode = versionCode;
