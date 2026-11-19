import { useCallback, useSyncExternalStore, type ReactNode } from "react";

export * from "@clerk/react";

/**
 * Takes @clerk/react's place on a local run when VITE_STAND_IN_CLERK is set, signing in one of
 * the stand-in sign-in's made-up people: the first, or the one `?as=` names. Signing out shows
 * the sign-in page, where any way in signs that person back in.
 */
type Person = { key: string; id: string; name: string };

const issuer = import.meta.env.VITE_STAND_IN_CLERK as string;
const people: Person[] = await (await fetch(`${issuer}/people`)).json();
const asked = new URLSearchParams(location.search).get("as");
if (asked) {
  localStorage.setItem("standInPerson", asked);
  localStorage.removeItem("standInSignedOut");
}
const signedIn = !localStorage.getItem("standInSignedOut");
const person =
  people.find((p) => p.key === localStorage.getItem("standInPerson")) ??
  people[0];

let held: { token: string; until: number } | null = null;
async function getToken() {
  if (held && held.until > Date.now()) return held.token;
  const response = await fetch(`${issuer}/token?as=${person.key}`);
  const { token } = (await response.json()) as { token: string };
  held = { token, until: Date.now() + 50 * 60_000 };
  return token;
}

type Raw = {
  primary_email_address_id: string;
  email_addresses: {
    id: string;
    email_address: string;
    verification: { status: string };
    linked_to: { type: string; id: string }[];
  }[];
  external_accounts: { id: string; provider: string; email_address: string }[];
};

const me = (path = "", init?: RequestInit) =>
  fetch(`${issuer}/me${path}?as=${person.key}`, {
    ...init,
    headers: init?.body ? { "content-type": "application/json" } : undefined,
  }).then(async (response) => {
    const body = await response.json();
    if (!response.ok) throw new Error(body.message ?? "The stand-in refused.");
    return body;
  });

const listeners = new Set<() => void>();
let user = shape(await me());

async function reload() {
  user = shape(await me());
  listeners.forEach((listener) => listener());
}

function address(raw: Raw["email_addresses"][number]) {
  return {
    id: raw.id,
    emailAddress: raw.email_address,
    verification: raw.verification,
    linkedTo: raw.linked_to,
    destroy: () => me(`/email_addresses/${raw.id}`, { method: "DELETE" }),
    prepareVerification: async () => address(raw),
    attemptVerification: ({ code }: { code: string }) =>
      me(`/email_addresses/${raw.id}/verify`, {
        method: "POST",
        body: JSON.stringify({ code }),
      }),
  };
}

function shape(raw: Raw) {
  const emailAddresses = raw.email_addresses.map(address);
  return {
    id: person.id,
    fullName: person.name,
    username: null,
    imageUrl: "",
    hasImage: false,
    primaryEmailAddressId: raw.primary_email_address_id,
    primaryEmailAddress:
      emailAddresses.find((a) => a.id === raw.primary_email_address_id) ?? null,
    emailAddresses,
    externalAccounts: raw.external_accounts.map((account) => ({
      id: account.id,
      provider: account.provider.replace(/^oauth_/, ""),
      emailAddress: account.email_address,
      destroy: () =>
        me(`/external_accounts/${account.id}`, { method: "DELETE" }),
    })),
    reload,
    createEmailAddress: async ({ email }: { email: string }) =>
      address(
        await me("/email_addresses", {
          method: "POST",
          body: JSON.stringify({ email }),
        }),
      ),
    createExternalAccount: async ({
      strategy,
      redirectUrl,
    }: {
      strategy: string;
      redirectUrl: string;
    }) => ({
      verification: {
        externalVerificationRedirectURL: new URL(
          `${issuer}/connect?as=${person.key}&provider=${strategy.replace(/^oauth_/, "")}&back=${encodeURIComponent(redirectUrl)}`,
        ),
      },
    }),
  };
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};

const auth = {
  isLoaded: true,
  isSignedIn: signedIn,
  userId: signedIn ? person.id : null,
  sessionId: signedIn ? `sess_${person.id}` : null,
  getToken,
};

export function ClerkProvider({ children }: { children: ReactNode }) {
  return children;
}

export const useAuth = () => auth;
export const useUser = () => {
  const current = useSyncExternalStore(subscribe, () => user);
  return {
    isLoaded: true,
    isSignedIn: signedIn,
    user: signedIn ? current : null,
  };
};
export const useSession = () => ({ isLoaded: true, session: null });
export const useClerk = () => ({
  signOut: async ({ redirectUrl = "/" }: { redirectUrl?: string } = {}) => {
    localStorage.setItem("standInSignedOut", "1");
    goFresh(redirectUrl);
  },
});

const signIn = {
  sso: async ({ redirectUrl }: { redirectUrl: string }) => {
    localStorage.removeItem("standInSignedOut");
    goFresh(redirectUrl);
    return { error: null };
  },
  reset: async () => undefined,
};
export const useSignIn = () => ({ signIn });
export const useSignUp = () => ({ signUp: {} });

/** Loads the page again even when only its #fragment differs, so the new sign-in state is read. */
function goFresh(url: string) {
  history.replaceState(null, "", url);
  location.reload();
}
export function useReverification<Fetcher>(fetcher: Fetcher) {
  return useCallback(fetcher as never, [fetcher]) as Fetcher;
}
