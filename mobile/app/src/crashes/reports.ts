import { type ComponentType, useEffect } from 'react';
import * as Sentry from '@sentry/react-native';
import { nativeApplicationVersion, nativeBuildVersion } from 'expo-application';
import * as Crypto from 'expo-crypto';
import * as Updates from 'expo-updates';

import { versionFromBuild } from '@/network/versions';

const DSN = 'https://76804485378be914417f459fe482c276@o4512409824919552.ingest.de.sentry.io/4512409866076240';

/** Only builds the release workflows make, which set EXPO_PUBLIC_SENTRY, send crash reports. */
export const reporting = !__DEV__ && process.env.EXPO_PUBLIC_SENTRY === '1';

/** Console lines and touched labels can hold what the person typed or read, so neither leaves the phone. */
/** Sentry works out a city from the sender's address unless the event names one, so every event names none. */
const NO_IP = '0.0.0.0';

const DROPPED_BREADCRUMBS = new Set(['console', 'touch']);

function keepBreadcrumb(breadcrumb: Sentry.Breadcrumb): Sentry.Breadcrumb | null {
  if (breadcrumb.category && DROPPED_BREADCRUMBS.has(breadcrumb.category)) return null;
  const url = breadcrumb.data?.url;
  if (typeof url === 'string') breadcrumb.data = { ...breadcrumb.data, url: url.split(/[?#]/, 1)[0] };
  return breadcrumb;
}

if (reporting) {
  Sentry.init({
    dsn: DSN,
    release: `sikemux-mobile@${versionFromBuild(nativeBuildVersion) ?? nativeApplicationVersion}+${nativeBuildVersion}`,
    environment: 'production',
    sendDefaultPii: false,
    tracesSampleRate: 0.05,
    // Frame and stall tracking double what init costs the first render.
    enableNativeFramesTracking: false,
    enableStallTracking: false,
    attachScreenshot: false,
    attachViewHierarchy: false,
    beforeBreadcrumb: keepBreadcrumb,
    initialScope: {
      user: { ip_address: NO_IP },
      tags: { channel: Updates.channel ?? 'none', commit: process.env.EXPO_PUBLIC_COMMIT ?? 'none' },
    },
  });
}

/** The root screen, wrapped so app start is measured from the first frame it draws. */
export function withCrashReports(Root: ComponentType<Record<string, unknown>>) {
  return reporting ? Sentry.wrap(Root) : Root;
}

/** Ends the app start measurement once the splash gives way to the first screen. */
export function appReady() {
  if (reporting) Sentry.appLoaded();
}

export function reportError(error: unknown) {
  if (reporting) Sentry.captureException(error);
}

/** Tags reports with the account the way the desktop's usage ping does: a hash, never the id itself. */
export function useCrashReportsAccount(userId: string | null | undefined) {
  useEffect(() => {
    if (!reporting) return;
    if (!userId) {
      Sentry.setUser({ ip_address: NO_IP });
      return;
    }
    let current = true;
    void Crypto.digestStringAsync(Crypto.CryptoDigestAlgorithm.SHA256, `sikemux-usage|${userId}`).then((hash) => {
      if (current) Sentry.setUser({ id: hash, ip_address: NO_IP });
    });
    return () => {
      current = false;
    };
  }, [userId]);
}
