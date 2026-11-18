import type { AccountProfile } from "@sikemux/protocol";

/** The calls the API makes to Clerk's Backend API. Each counts "already gone" as done. */
export interface ClerkBackend {
  deleteUser(userId: string): Promise<void>;
  /** The name, primary email and picture the user signed in with; all null once the user is gone. */
  profile(userId: string): Promise<Omit<AccountProfile, "staff">>;
  /** The user's primary email once Clerk has verified it, else null. */
  verifiedEmail(userId: string): Promise<string | null>;
  /** The domains of every address on the user that Clerk has verified, lowercased; the addresses themselves never leave here. */
  verifiedDomains(userId: string): Promise<string[]>;
  /** When the user was made and their primary address once verified; null once the user is gone. */
  signUp(userId: string): Promise<SignUp | null>;
  /** The names and pictures of up to 100 users, by id; a user who is gone is left out. */
  people(userIds: string[]): Promise<Map<string, Person>>;
  revokeSession(sessionId: string): Promise<void>;
  /** The access tokens Clerk holds from the user's web sign-ins with Apple; none for the phone's. */
  appleAccessTokens(userId: string): Promise<string[]>;
  /** Every address and connected account on the user, for the API's own checks; null once the user is gone. Never log what it holds. */
  signIns(userId: string): Promise<SignIns | null>;
  setPrimaryAddress(userId: string, addressId: string): Promise<void>;
  removeAddress(addressId: string): Promise<void>;
  removeExternalAccount(userId: string, accountId: string): Promise<void>;
}

export type SignIns = {
  primaryAddressId: string | null;
  addresses: { id: string; email: string; verified: boolean; addedAt: Date }[];
  externalAccounts: { id: string; provider: string; email: string }[];
};

export type Person = Pick<AccountProfile, "name" | "imageUrl">;

export interface SignUp {
  createdAt: Date;
  email: string | null;
}

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
  created_at?: number;
  primary_email_address_id?: string | null;
  email_addresses?: {
    id?: string;
    email_address?: string;
    created_at?: number;
    verification?: { status?: string } | null;
  }[];
  external_accounts?: {
    id?: string;
    provider?: string;
    email_address?: string;
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
  const request = async (method: string, path: string, body?: object) => {
    const response = await fetcher(`${base}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${secretKey}`,
        ...(body ? { "content-type": "application/json" } : {}),
      },
      ...(body ? { body: JSON.stringify(body) } : {}),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    if (response.ok) return response;
    await response.body?.cancel();
    if (response.status === 404) return null;
    throw new Error(`Clerk answered ${response.status} to ${method} ${path}`);
  };
  const call = async (method: string, path: string, body?: object) => {
    const response = await request(method, path, body);
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
    const address = user?.email_addresses?.find(
      (address) => address.id === user.primary_email_address_id,
    );
    return {
      user,
      address,
      verified:
        address?.verification?.status === "verified"
          ? (address.email_address ?? null)
          : null,
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
    verifiedEmail: async (userId) => (await primaryEmail(userId)).verified,
    signUp: async (userId) => {
      const { user, verified } = await primaryEmail(userId);
      if (typeof user?.created_at !== "number") return null;
      return { createdAt: new Date(user.created_at), email: verified };
    },
    verifiedDomains: async (userId) => {
      const { user } = await primaryEmail(userId);
      const domains = (user?.email_addresses ?? []).flatMap((address) => {
        const email = address.email_address ?? "";
        const at = email.lastIndexOf("@");
        return address.verification?.status === "verified" && at > 0
          ? [email.slice(at + 1).toLowerCase()]
          : [];
      });
      return [...new Set(domains)];
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
    signIns: async (userId) => {
      const { user } = await primaryEmail(userId);
      if (!user) return null;
      return {
        primaryAddressId: user.primary_email_address_id ?? null,
        addresses: (user.email_addresses ?? []).flatMap((address) =>
          address.id && address.email_address
            ? [
                {
                  id: address.id,
                  email: address.email_address,
                  verified: address.verification?.status === "verified",
                  addedAt: new Date(address.created_at ?? 0),
                },
              ]
            : [],
        ),
        externalAccounts: (user.external_accounts ?? []).flatMap((account) =>
          account.id && account.provider
            ? [
                {
                  id: account.id,
                  provider: account.provider.replace(/^oauth_/, ""),
                  email: account.email_address ?? "",
                },
              ]
            : [],
        ),
      };
    },
    setPrimaryAddress: async (userId, addressId) =>
      call("PATCH", `/v1/users/${checked(userId, "user")}`, {
        primary_email_address_id: checked(addressId, "idn"),
      }),
    removeAddress: async (addressId) =>
      call("DELETE", `/v1/email_addresses/${checked(addressId, "idn")}`),
    removeExternalAccount: async (userId, accountId) =>
      call(
        "DELETE",
        `/v1/users/${checked(userId, "user")}/external_accounts/${checked(accountId, "(?:idn|eac)")}`,
      ),
  };
}
