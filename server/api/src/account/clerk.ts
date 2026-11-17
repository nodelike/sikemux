import type { AccountProfile } from "@sikemux/protocol";

/** The calls the API makes to Clerk's Backend API. Each counts "already gone" as done. */
export interface ClerkBackend {
  deleteUser(userId: string): Promise<void>;
  /** The name, primary email and picture the user signed in with; all null once the user is gone. */
  profile(userId: string): Promise<Omit<AccountProfile, "staff">>;
  /** The user's primary email once Clerk has verified it, else null. */
  verifiedEmail(userId: string): Promise<string | null>;
  /** The names and pictures of up to 100 users, by id; a user who is gone is left out. */
  people(userIds: string[]): Promise<Map<string, Person>>;
  revokeSession(sessionId: string): Promise<void>;
  /** The access tokens Clerk holds from the user's web sign-ins with Apple; none for the phone's. */
  appleAccessTokens(userId: string): Promise<string[]>;
}

export type Person = Pick<AccountProfile, "name" | "imageUrl">;

type Fetch = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

export const CLERK_API = "https://api.clerk.com";

interface ClerkUser {
  id?: string;
  first_name?: string | null;
  last_name?: string | null;
  image_url?: string | null;
  primary_email_address_id?: string | null;
  email_addresses?: {
    id?: string;
    email_address?: string;
    verification?: { status?: string } | null;
  }[];
}
const TIMEOUT_MS = 10_000;

function personOf(user: ClerkUser | undefined): Person {
  const name = [user?.first_name, user?.last_name]
    .filter((part) => part?.trim())
    .join(" ");
  return {
    name: name.slice(0, 256) || null,
    imageUrl: user?.image_url?.startsWith("https://")
      ? user.image_url.slice(0, 2048)
      : null,
  };
}

export function clerkBackend(
  secretKey: string,
  fetcher: Fetch = fetch,
  base = CLERK_API,
): ClerkBackend {
  const request = async (method: string, path: string) => {
    const response = await fetcher(`${base}${path}`, {
      method,
      headers: { authorization: `Bearer ${secretKey}` },
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    if (response.ok) return response;
    await response.body?.cancel();
    if (response.status === 404) return null;
    throw new Error(`Clerk answered ${response.status} to ${method} ${path}`);
  };
  const call = async (method: string, path: string) => {
    const response = await request(method, path);
    await response?.body?.cancel();
  };
  const checked = (id: string, prefix: string) => {
    if (!new RegExp(`^${prefix}_[A-Za-z0-9]+$`).test(id))
      throw new Error(`${JSON.stringify(id)} is not a Clerk ${prefix} id`);
    return id;
  };
  const primaryEmail = async (userId: string) => {
    const response = await request(
      "GET",
      `/v1/users/${checked(userId, "user")}`,
    );
    const user = (await response?.json()) as ClerkUser | undefined;
    return {
      user,
      address: user?.email_addresses?.find(
        (address) => address.id === user.primary_email_address_id,
      ),
    };
  };
  return {
    deleteUser: async (userId) =>
      call("DELETE", `/v1/users/${checked(userId, "user")}`),
    profile: async (userId) => {
      const { user, address } = await primaryEmail(userId);
      const email = address?.email_address?.slice(0, 320) ?? null;
      return { email, ...personOf(user) };
    },
    verifiedEmail: async (userId) => {
      const { address } = await primaryEmail(userId);
      return address?.verification?.status === "verified"
        ? (address.email_address ?? null)
        : null;
    },
    people: async (userIds) => {
      const query = new URLSearchParams({ limit: "100" });
      for (const id of userIds) query.append("user_id", checked(id, "user"));
      const response = await request("GET", `/v1/users?${query}`);
      const listed: unknown = await response?.json();
      const users = Array.isArray(listed) ? (listed as ClerkUser[]) : [];
      return new Map(
        users.flatMap((user) =>
          typeof user.id === "string" ? [[user.id, personOf(user)]] : [],
        ),
      );
    },
    revokeSession: async (sessionId) =>
      call("POST", `/v1/sessions/${checked(sessionId, "sess")}/revoke`),
    appleAccessTokens: async (userId) => {
      const response = await request(
        "GET",
        `/v1/users/${checked(userId, "user")}/oauth_access_tokens/oauth_apple`,
      );
      const listed: unknown = await response?.json();
      if (!Array.isArray(listed)) return [];
      return listed.flatMap((entry: { token?: unknown }) =>
        typeof entry?.token === "string" && entry.token ? [entry.token] : [],
      );
    },
  };
}
