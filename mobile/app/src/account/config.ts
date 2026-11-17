import release from '@sikemux/protocol/release.json';
import Constants from 'expo-constants';

/** How long a deleted account can be restored, and how long backups outlive it after that. */
export const RETENTION = release.account;

const extra = Constants.expoConfig?.extra ?? {};

/**
 * The build's variant picks the accounts, not whether it runs from Metro: a production build signs in to
 * the real accounts even while it hot-reloads. Clerk's publishable key is public.
 */
export const CLERK_PUBLISHABLE_KEY: string = extra.clerkPublishableKey;

/** A development build uses the accounts server on the Mac running Metro, or EXPO_PUBLIC_API_URL when it is set. */
export function apiUrl(): string {
  if (typeof extra.apiUrl === 'string') return extra.apiUrl;
  const configured = process.env.EXPO_PUBLIC_API_URL;
  if (configured) return configured.replace(/\/$/, '');
  const metro = Constants.expoConfig?.hostUri?.split(':')[0];
  return `http://${metro ?? '127.0.0.1'}:${release.accounts.dev.apiPort}`;
}
