const fs = require('node:fs');
const path = require('node:path');
const {
  AndroidConfig,
  withAndroidManifest,
  withDangerousMod,
  withEntitlementsPlist,
  withInfoPlist,
  withXcodeProject,
} = require('expo/config-plugins');

const EXTENSION = 'SikemuxShare';
const MODULE = path.join(__dirname, '..', 'modules', 'stash', 'ios');
const NOTCH = path.join(__dirname, '..', '..', '..', 'helpers', 'notch', 'Sources', 'NotchKit');
const EXTENSION_SOURCES = [
  path.join(MODULE, 'Extension', 'ShareViewController.swift'),
  path.join(MODULE, 'Extension', 'ShareModel.swift'),
  path.join(MODULE, 'Extension', 'ShareView.swift'),
  path.join(MODULE, 'Extension', 'StashUpload.swift'),
  path.join(MODULE, 'Extension', 'Theme.swift'),
  path.join(MODULE, 'Shared', 'StashShared.swift'),
  path.join(NOTCH, 'IconView.swift'),
  path.join(NOTCH, 'Icons.generated.swift'),
];
const SHARED_TYPES = ['text/plain', 'image/*', 'video/*', '*/*'];

function escape(text) {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function plistValue(value, indent) {
  const inner = `${indent}  `;
  if (Array.isArray(value)) return `<array>\n${value.map((item) => `${inner}${plistValue(item, inner)}\n`).join('')}${indent}</array>`;
  if (typeof value === 'boolean') return value ? '<true/>' : '<false/>';
  if (typeof value === 'number') return `<integer>${value}</integer>`;
  if (typeof value === 'object') {
    const entries = Object.entries(value).map(([key, item]) => `${inner}<key>${escape(key)}</key>\n${inner}${plistValue(item, inner)}\n`);
    return `<dict>\n${entries.join('')}${indent}</dict>`;
  }
  return `<string>${escape(String(value))}</string>`;
}

function plist(value) {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
${plistValue(value, '')}
</plist>
`;
}

/** The App Group the app writes the share extension's chats into. */
function appGroup(config) {
  return `group.${config.ios.bundleIdentifier}`;
}

/** The Keychain group the app keeps a copy of the phone's key in, which the share extension signs with. */
function keychainGroup(config) {
  // Not $(AppIdentifierPrefix): Info.plist expands it to nothing, so the code would name a group it isn't entitled to.
  return `${config.ios.appleTeamId}.${config.ios.bundleIdentifier}.stash`;
}

// Another app's share menu opens the app on its share screen, for text, links, pictures, videos and any file.
const withShareIntents = (config) =>
  withAndroidManifest(config, (config) => {
    const activity = AndroidConfig.Manifest.getMainActivityOrThrow(config.modResults);
    const filters = (activity['intent-filter'] ??= []);
    const sharing = (filter) => filter.action?.some((action) => action.$['android:name'].startsWith('android.intent.action.SEND'));
    activity['intent-filter'] = filters.filter((filter) => !sharing(filter));
    for (const action of ['android.intent.action.SEND', 'android.intent.action.SEND_MULTIPLE']) {
      activity['intent-filter'].push({
        action: [{ $: { 'android:name': action } }],
        category: [{ $: { 'android:name': 'android.intent.category.DEFAULT' } }],
        data: SHARED_TYPES.map((type) => ({ $: { 'android:mimeType': type } })),
      });
    }
    return config;
  });

// Sikemux in the iOS share menu is a share extension; it reads the app's chats from their App Group and signs as
// the phone with the key the app shares through a Keychain group.
const withShareExtension = (config) => {
  config = withEntitlementsPlist(config, (config) => {
    config.modResults['com.apple.security.application-groups'] = [appGroup(config)];
    const groups = new Set(config.modResults['keychain-access-groups'] ?? []);
    groups.add(keychainGroup(config));
    config.modResults['keychain-access-groups'] = [...groups];
    return config;
  });
  config = withInfoPlist(config, (config) => {
    config.modResults.SikemuxAppGroup = appGroup(config);
    config.modResults.SikemuxStashKeychainGroup = keychainGroup(config);
    return config;
  });
  config = withDangerousMod(config, [
    'ios',
    (config) => {
      const folder = path.join(config.modRequest.platformProjectRoot, EXTENSION);
      fs.mkdirSync(folder, { recursive: true });
      for (const source of EXTENSION_SOURCES) fs.copyFileSync(source, path.join(folder, path.basename(source)));
      const development = config.ios.bundleIdentifier.endsWith('.dev');
      fs.writeFileSync(
        path.join(folder, `${EXTENSION}-Info.plist`),
        plist({
          CFBundleDevelopmentRegion: '$(DEVELOPMENT_LANGUAGE)',
          CFBundleDisplayName: config.name,
          CFBundleExecutable: '$(EXECUTABLE_NAME)',
          CFBundleIdentifier: '$(PRODUCT_BUNDLE_IDENTIFIER)',
          CFBundleInfoDictionaryVersion: '6.0',
          CFBundleName: '$(PRODUCT_NAME)',
          CFBundlePackageType: '$(PRODUCT_BUNDLE_PACKAGE_TYPE)',
          CFBundleShortVersionString: config.version,
          CFBundleVersion: config.ios.buildNumber ?? '1',
          SikemuxAppGroup: appGroup(config),
          SikemuxStashKeychainGroup: keychainGroup(config),
          // A development build reaches the accounts server on the Mac running Metro, over plain HTTP.
          NSAppTransportSecurity: development ? { NSAllowsArbitraryLoads: true } : { NSAllowsLocalNetworking: true },
          NSExtension: {
            NSExtensionPointIdentifier: 'com.apple.share-services',
            NSExtensionPrincipalClass: '$(PRODUCT_MODULE_NAME).ShareViewController',
            NSExtensionAttributes: {
              NSExtensionActivationRule: {
                NSExtensionActivationSupportsText: true,
                NSExtensionActivationSupportsWebURLWithMaxCount: 1,
                NSExtensionActivationSupportsImageWithMaxCount: 10,
                NSExtensionActivationSupportsMovieWithMaxCount: 4,
                NSExtensionActivationSupportsFileWithMaxCount: 10,
                NSExtensionActivationSupportsAttachmentsWithMaxCount: 10,
              },
            },
          },
        }),
      );
      fs.writeFileSync(
        path.join(folder, `${EXTENSION}.entitlements`),
        plist({ 'com.apple.security.application-groups': [appGroup(config)], 'keychain-access-groups': [keychainGroup(config)] }),
      );
      return config;
    },
  ]);
  return withXcodeProject(config, (config) => {
    const project = config.modResults;
    if (project.pbxTargetByName(EXTENSION)) return config;
    const objects = project.hash.project.objects;
    objects.PBXTargetDependency ??= {};
    objects.PBXContainerItemProxy ??= {};

    const appSettings = Object.values(project.pbxXCBuildConfigurationSection()).find(
      (entry) => entry.buildSettings?.PRODUCT_BUNDLE_IDENTIFIER && entry.buildSettings.IPHONEOS_DEPLOYMENT_TARGET,
    )?.buildSettings;
    const target = project.addTarget(EXTENSION, 'app_extension', EXTENSION, `${config.ios.bundleIdentifier}.share`);
    const files = EXTENSION_SOURCES.map((source) => path.basename(source));
    const group = project.addPbxGroup([...files, `${EXTENSION}-Info.plist`, `${EXTENSION}.entitlements`], EXTENSION, EXTENSION);
    project.addToPbxGroup(group.uuid, project.getFirstProject().firstProject.mainGroup);
    project.addBuildPhase(files, 'PBXSourcesBuildPhase', 'Sources', target.uuid);
    project.addBuildPhase([], 'PBXResourcesBuildPhase', 'Resources', target.uuid);
    project.addFramework('LinkPresentation.framework', { target: target.uuid, link: true });

    for (const entry of Object.values(project.pbxXCBuildConfigurationSection())) {
      if (entry.buildSettings?.PRODUCT_NAME !== `"${EXTENSION}"`) continue;
      Object.assign(entry.buildSettings, {
        CODE_SIGN_ENTITLEMENTS: `${EXTENSION}/${EXTENSION}.entitlements`,
        CODE_SIGN_STYLE: 'Automatic',
        IPHONEOS_DEPLOYMENT_TARGET: appSettings?.IPHONEOS_DEPLOYMENT_TARGET ?? '17.0',
        SWIFT_VERSION: '5.0',
        TARGETED_DEVICE_FAMILY: '1',
        ...(config.ios.appleTeamId ? { DEVELOPMENT_TEAM: config.ios.appleTeamId } : {}),
      });
    }
    if (config.ios.appleTeamId) project.addTargetAttribute('DevelopmentTeam', config.ios.appleTeamId, target);
    return config;
  });
};

module.exports = (config) => withShareExtension(withShareIntents(config));
