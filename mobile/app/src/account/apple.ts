import { Platform } from 'react-native';
import * as AppleAuthentication from 'expo-apple-authentication';

type AppleConsent = { cancelled: true } | { cancelled: false; code?: string };

/**
 * Before an account that signs in with Apple is deleted, shows Apple's sheet for a fresh
 * authorization code, which the server revokes so Apple forgets the app too. Closing the sheet
 * cancels the deletion; when Apple can't give a code, the deletion goes on without one.
 */
export async function appleConsent(accounts: readonly { provider: string }[] | undefined): Promise<AppleConsent> {
  if (Platform.OS !== 'ios' || !accounts?.some((account) => account.provider === 'apple')) return { cancelled: false };
  try {
    if (!(await AppleAuthentication.isAvailableAsync())) return { cancelled: false };
    const credential = await AppleAuthentication.signInAsync({ requestedScopes: [] });
    return { cancelled: false, code: credential.authorizationCode ?? undefined };
  } catch (error) {
    if ((error as { code?: unknown } | null)?.code === 'ERR_REQUEST_CANCELED') return { cancelled: true };
    return { cancelled: false };
  }
}
