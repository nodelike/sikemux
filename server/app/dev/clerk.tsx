import { useCallback, type ReactNode } from "react";

export * from "@clerk/react";

/**
 * Takes @clerk/react's place on a local run when VITE_STAND_IN_CLERK is set, signing in one of
 * the stand-in sign-in's made-up people: the first, or the one `?as=` names.
 */
type Person = { key: string; id: string; name: string };

const issuer = import.meta.env.VITE_STAND_IN_CLERK as string;
const people: Person[] = await (await fetch(`${issuer}/people`)).json();
const asked = new URLSearchParams(location.search).get("as");
if (asked) localStorage.setItem("standInPerson", asked);
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

const user = {
  id: person.id,
  fullName: person.name,
  username: null,
  imageUrl: "",
  hasImage: false,
  primaryEmailAddress: null,
  externalAccounts: [{ provider: "google" }],
};

const auth = {
  isLoaded: true,
  isSignedIn: true,
  userId: person.id,
  sessionId: `sess_${person.id}`,
  getToken,
};

export function ClerkProvider({ children }: { children: ReactNode }) {
  return children;
}

export const useAuth = () => auth;
export const useUser = () => ({ isLoaded: true, isSignedIn: true, user });
export const useSession = () => ({ isLoaded: true, session: null });
export const useClerk = () => ({
  signOut: async () => location.assign("/"),
});
export function useReverification<Fetcher>(fetcher: Fetcher) {
  return useCallback(fetcher as never, [fetcher]) as Fetcher;
}
