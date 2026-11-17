import * as Application from 'expo-application';
import { Platform } from 'react-native';

import { versionFromBuild } from './versions';

export type Release = { version: string; platform: 'android' };

/**
 * The installed app's release, which an over-the-air update on top of it does not change. Development
 * builds have none, so they are never too old.
 */
export function installedRelease(): Release | null {
  if (__DEV__) return null;
  const version = versionFromBuild(Application.nativeBuildVersion);
  return version ? { version, platform: 'android' } : null;
}

/** Where a person gets a newer app: the APK on GitHub, or the site off Android. */
export function updateLink(): string {
  return Platform.OS === 'android' ? 'https://github.com/nodelike/sikemux/releases' : 'https://sikemux.com/';
}
