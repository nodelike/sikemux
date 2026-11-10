import * as SecureStore from 'expo-secure-store';
import type { TokenCache } from '@clerk/expo';

/** Clerk's sign-in, kept in this phone's Keystore. */
export const tokenCache: TokenCache = {
  getToken: (key) => SecureStore.getItemAsync(key).catch(() => null),
  saveToken: (key, token) => SecureStore.setItemAsync(key, token),
  clearToken: (key) => SecureStore.deleteItemAsync(key),
};
