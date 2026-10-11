/*
 * In the showcase the phone is signed in to a demo account, so it opens on the Mac
 * the demo world pretends to be instead of the welcome screen.
 */
import type { ReactNode } from 'react';

const user = {
  id: 'user_showcase',
  firstName: 'Edon',
  lastName: null,
  fullName: 'Edon',
  imageUrl: '',
  hasImage: false,
  primaryEmailAddress: { emailAddress: 'edon@acme.dev' },
  emailAddresses: [{ emailAddress: 'edon@acme.dev' }],
  externalAccounts: [],
  passwordEnabled: false,
};

const auth = {
  isLoaded: true,
  isSignedIn: true,
  userId: user.id,
  sessionId: 'sess_showcase',
  getToken: async () => 'showcase',
  signOut: async () => {},
};

export function ClerkProvider({ children }: { children: ReactNode }) {
  return children;
}

export const useAuth = () => auth;
export const useUser = () => ({ isLoaded: true, isSignedIn: true, user });
export const useSession = () => ({ isLoaded: true, isSignedIn: true, session: null });
export const useReverification = <T,>(action: T) => action;
export const useSSO = () => ({ startSSOFlow: async () => ({ createdSessionId: null }) });
export const useSignIn = () => ({ isLoaded: true, signIn: null, setActive: async () => {} });
export const useSignUp = () => ({ isLoaded: true, signUp: null, setActive: async () => {} });
export const useSignInWithApple = () => ({ startAppleAuthenticationFlow: async () => ({ createdSessionId: null }) });
export const useSignInWithGoogle = () => ({ startGoogleAuthenticationFlow: async () => ({ createdSessionId: null }) });
export const tokenCache = undefined;
export const resourceCache = undefined;
