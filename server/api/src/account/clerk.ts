import { randomBytes } from "node:crypto";
import type { AccountProfile } from "@sikemux/protocol";

import { domainOf } from "./proofs.ts";

/** The calls the API makes to Clerk's Backend API. Each counts "already gone" as done. */
export interface ClerkBackend {
  deleteUser(userId: string): Promise<void>;
  /** The name, primary email and picture the user signed in with; all null once the user is gone. */
  profile(userId: string): Promise<Omit<AccountProfile, "staff">>;
  /** The user's primary email once Clerk has verified it, else null. */
  verifiedEmail(userId: string): Promise<string | null>;
  /** The domains of every address on the user that Clerk has verified, lowercased; the addresses themselves never leave here. */
  verifiedDomains(userId: string): Promise<string[]>;
  /** When the user was made and the addresses it signs in with, main first; null once the user is gone. */
  signUp(userId: string): Promise<SignUp | null>;
  /** The names and pictures of up to 100 users, by id; a user who is gone is left out. */
  people(userIds: string[]): Promise<Map<string, Person>>;
  revokeSession(sessionId: string): Promise<void>;
  /** Whether Clerk says the session can no longer be used, or has no such session. Throws on an answer it can't read. */
  sessionOver(sessionId: string): Promise<boolean>;
  /** Every session of the user, ended ones too, with when each was made and whether it can still be used. Throws on an answer it can't read. */
  sessions(userId: string): Promise<ClerkSession[]>;
  /** The access tokens Clerk holds from the user's web sign-ins with Apple; none for the phone's. */
  appleAccessTokens(userId: string): Promise<string[]>;
  /** Every address and connected account on the user, for the API's own checks; null once the user is gone. Never log what it holds. */
  signIns(userId: string): Promise<SignIns | null>;
  setPrimaryAddress(userId: string, addressId: string): Promise<void>;
  removeAddress(addressId: string): Promise<void>;
  removeExternalAccount(userId: string, accountId: string): Promise<void>;
  removePhone(phoneId: string): Promise<void>;
  removeWallet(userId: string, walletId: string): Promise<void>;
  /** Sets a long random password nobody keeps, so the one before stops working. */
  replacePassword(userId: string): Promise<void>;
  /** The user, if it carries the demo mark and its main address is verified; null otherwise or once it is gone. */
  demoAccount(userId: string): Promise<DemoAccount | null>;
  /** Whether this is the user's password; false when the user has none or is gone. Never log the password. */
  passwordMatches(userId: string, password: string): Promise<boolean>;
  /** A token that signs the user in once by Clerk's ticket strategy, without a password or a new-device check. */
  signInToken(userId: string, expiresInSeconds: number): Promise<string>;
}

export type SignIns = {
  primaryAddressId: string | null;
  addresses: {
    id: string;
    email: string;
    signsIn: boolean;
    verified: boolean;
    addedAt: Date;
  }[];
  externalAccounts: {
    id: string;
    provider: string;
    email: string;
    addedAt: Date;
  }[];
  phones: { id: string; signsIn: boolean; addedAt: Date }[];
  wallets: { id: string; addedAt: Date }[];
  /** Null too for a password Clerk keeps no time for, which is in `undated`. */
  passwordSetAt: Date | null;
  /** The kinds of sign-in the account holds that Clerk keeps no time for. */
  undated: string[];
};

/** Clerk turned a request down, as against failing to answer it. */
export class ClerkRefusal extends Error {}

export type ClerkSession = { id: string; createdAt: Date; live: boolean };

/** The session statuses Clerk documents as over; any other, one it adds later included, counts as live. */
const OVER = new Set([
  "ended",
  "expired",
  "removed",
  "replaced",
  "revoked",
  "abandoned",
]);

/**
 * Ends a session at Clerk. A refusal counts as done once Clerk says the session is over or gone,
 * as when it ran out before it could be revoked.
 */
export async function endSessionInClerk(
  clerk: ClerkBackend,
  sessionId: string,
): Promise<void> {
  try {
    await clerk.revokeSession(sessionId);
  } catch (error) {
    if (!(error instanceof ClerkRefusal)) throw error;
    if (!(await clerk.sessionOver(sessionId))) throw error;
  }
}

/** Whether Clerk itself said it has no such thing, rather than a 404 from something in between. */
async function clerkFoundNothing(response: Response): Promise<boolean> {
  if (response.status !== 404) return false;
  const answer = (await response.json().catch(() => null)) as {
    errors?: { code?: unknown }[];
  } | null;
  return (
    answer?.errors?.some((error) => error.code === "resource_not_found") ===
    true
  );
}

/** The mark in a Clerk user's private metadata that makes it the store reviewers' demo account. */
export const DEMO_MARK = "storeReviewDemo";

export interface DemoAccount {
  /** The verified main address reviewers sign in with. */
  address: string;
  /** Every address that signs in to the user: verified, or reserved by Clerk. */
  addresses: string[];
  passwordSetAt: number | null;
  secondFactor: boolean;
}

export type Person = Pick<AccountProfile, "name" | "imageUrl">;

export interface SignUp {
  createdAt: Date;
  /** The main address, once Clerk has verified it. */
  email: string | null;
  /** Every address the user can sign in with, the main one first. */
  addresses: { email: string; verified: boolean; addedAt: Date }[];
}

type Fetch = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

/** Clerk lets a reserved address sign in before it is verified. */
export function signsIn(address: {
  reserved?: boolean;
  verification?: { status?: string } | null;
}) {
  return (
    address.verification?.status === "verified" || address.reserved === true
  );
}

export const CLERK_API = "https://api.clerk.com";

interface ClerkUser {
  id?: string;
  first_name?: string | null;
  last_name?: string | null;
  image_url?: string | null;
  created_at?: number;
  primary_email_address_id?: string | null;
  private_metadata?: Record<string, unknown> | null;
  two_factor_enabled?: boolean;
  email_addresses?: {
    id?: string;
    email_address?: string;
    created_at?: number;
    reserved?: boolean;
    verification?: { status?: string } | null;
  }[];
  external_accounts?: {
    external_account_id?: string;
    provider?: string;
    email_address?: string;
    created_at?: number;
  }[];
  phone_numbers?: {
    id?: string;
    created_at?: number;
    reserved_for_second_factor?: boolean;
    verification?: { status?: string } | null;
  }[];
  web3_wallets?: { id?: string; created_at?: number }[];
  passkeys?: unknown[];
  saml_accounts?: unknown[];
  enterprise_accounts?: unknown[];
  totp_enabled?: boolean;
  backup_code_enabled?: boolean;
  username?: string | null;
  password_enabled?: boolean;
  password_last_updated_at?: number | null;
}
const TIMEOUT_MS = 10_000;
const SESSIONS_PAGE = 500;

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

/** A path with its ids and query hidden, so a logged failure names no person. */
const withoutIds = (path: string) =>
  path
    .replace(/\?.*$/, "")
    .replace(/\b(user|idn|sess|eac)_[A-Za-z0-9]+/g, "$1_…");

export function clerkBackend(
  secretKey: string,
  fetcher: Fetch = fetch,
  base = CLERK_API,
): ClerkBackend {
  const request = async (
    method: string,
    path: string,
    body?: object,
    refusals: readonly number[] | ((response: Response) => Promise<boolean>) = [
      404,
    ],
  ) => {
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
    if (typeof refusals === "function") {
      const refused = await refusals(response);
      await response.body?.cancel().catch(() => undefined);
      if (refused) return null;
    } else {
      await response.body?.cancel();
      if (refusals.includes(response.status)) return null;
    }
    const said = `Clerk answered ${response.status} to ${method} ${withoutIds(path)}`;
    throw response.status < 500 ? new ClerkRefusal(said) : new Error(said);
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
    deleteUser: async (userId) => {
      const response = await request(
        "DELETE",
        `/v1/users/${checked(userId, "user")}`,
        undefined,
        clerkFoundNothing,
      );
      await response?.body?.cancel();
    },
    profile: async (userId) => {
      const { user, address } = await primaryEmail(userId);
      const email = address?.email_address?.slice(0, 320) ?? null;
      return { email, ...personOf(user) };
    },
    verifiedEmail: async (userId) => (await primaryEmail(userId)).verified,
    signUp: async (userId) => {
      const { user, address: main, verified } = await primaryEmail(userId);
      if (typeof user?.created_at !== "number") return null;
      const createdAt = new Date(user.created_at);
      const addresses = (user.email_addresses ?? []).flatMap((address) =>
        signsIn(address) && address.email_address
          ? [
              {
                email: address.email_address,
                verified: address.verification?.status === "verified",
                addedAt:
                  typeof address.created_at === "number"
                    ? new Date(
                        Math.max(address.created_at, createdAt.getTime()),
                      )
                    : createdAt,
              },
            ]
          : [],
      );
      return {
        createdAt,
        email: verified,
        addresses: addresses.sort(
          (a, b) =>
            Number(b.email === main?.email_address) -
            Number(a.email === main?.email_address),
        ),
      };
    },
    verifiedDomains: async (userId) => {
      const { user } = await primaryEmail(userId);
      const domains = (user?.email_addresses ?? []).flatMap((address) => {
        const domain = domainOf(address.email_address ?? "");
        return address.verification?.status === "verified" && domain
          ? [domain]
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
    revokeSession: async (sessionId) => {
      const response = await request(
        "POST",
        `/v1/sessions/${checked(sessionId, "sess")}/revoke`,
        undefined,
        clerkFoundNothing,
      );
      await response?.body?.cancel();
    },
    sessionOver: async (sessionId) => {
      const response = await request(
        "GET",
        `/v1/sessions/${checked(sessionId, "sess")}`,
        undefined,
        clerkFoundNothing,
      );
      if (!response) return true;
      const session = (await response.json()) as { status?: unknown } | null;
      if (typeof session?.status !== "string")
        throw new Error("Clerk answered a session it didn't describe");
      return OVER.has(session.status);
    },
    sessions: async (userId) => {
      const sessions: ClerkSession[] = [];
      for (let offset = 0; ; offset += SESSIONS_PAGE) {
        const query = new URLSearchParams({
          user_id: checked(userId, "user"),
          limit: String(SESSIONS_PAGE),
          offset: String(offset),
        });
        const response = await request("GET", `/v1/sessions?${query}`);
        const listed: unknown = await response?.json();
        const page = Array.isArray(listed)
          ? listed
          : (listed as { data?: unknown } | undefined)?.data;
        if (!Array.isArray(page))
          throw new Error("Clerk's list of sessions was not a list");
        for (const session of page as {
          id?: unknown;
          status?: unknown;
          created_at?: unknown;
        }[]) {
          if (
            typeof session?.id !== "string" ||
            typeof session.created_at !== "number" ||
            typeof session.status !== "string"
          )
            throw new Error("Clerk listed a session it didn't describe");
          sessions.push({
            id: session.id,
            createdAt: new Date(session.created_at),
            live: !OVER.has(session.status),
          });
        }
        if (page.length < SESSIONS_PAGE) return sessions;
      }
    },
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
                  signsIn: signsIn(address),
                  verified: address.verification?.status === "verified",
                  addedAt: new Date(address.created_at ?? 0),
                },
              ]
            : [],
        ),
        externalAccounts: (user.external_accounts ?? []).flatMap((account) =>
          account.external_account_id && account.provider
            ? [
                {
                  id: account.external_account_id,
                  provider: account.provider.replace(/^oauth_/, ""),
                  email: account.email_address ?? "",
                  addedAt: new Date(account.created_at ?? 0),
                },
              ]
            : [],
        ),
        phones: (user.phone_numbers ?? []).flatMap((phone) =>
          phone.id
            ? [
                {
                  id: phone.id,
                  signsIn:
                    phone.verification?.status === "verified" &&
                    !phone.reserved_for_second_factor,
                  addedAt: new Date(phone.created_at ?? 0),
                },
              ]
            : [],
        ),
        wallets: (user.web3_wallets ?? []).flatMap((wallet) =>
          wallet.id
            ? [{ id: wallet.id, addedAt: new Date(wallet.created_at ?? 0) }]
            : [],
        ),
        passwordSetAt:
          user.password_enabled &&
          typeof user.password_last_updated_at === "number"
            ? new Date(user.password_last_updated_at)
            : null,
        undated: [
          ...(user.passkeys?.length ? ["passkey"] : []),
          ...(user.saml_accounts?.length ? ["SAML account"] : []),
          ...(user.enterprise_accounts?.length ? ["enterprise account"] : []),
          ...(user.totp_enabled ? ["authenticator app"] : []),
          ...(user.backup_code_enabled ? ["backup codes"] : []),
          ...(user.username ? ["username"] : []),
          ...(user.password_enabled &&
          typeof user.password_last_updated_at !== "number"
            ? ["password"]
            : []),
        ],
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
        `/v1/users/${checked(userId, "user")}/external_accounts/${checked(accountId, "eac")}`,
      ),
    removePhone: async (phoneId) =>
      call("DELETE", `/v1/phone_numbers/${checked(phoneId, "idn")}`),
    removeWallet: async (userId, walletId) =>
      call(
        "DELETE",
        `/v1/users/${checked(userId, "user")}/web3_wallets/${checked(walletId, "idn")}`,
      ),
    replacePassword: async (userId) =>
      call("PATCH", `/v1/users/${checked(userId, "user")}`, {
        password: randomBytes(48).toString("base64url"),
        skip_password_checks: true,
      }),
    demoAccount: async (userId) => {
      const { user, verified } = await primaryEmail(userId);
      if (!verified || user?.private_metadata?.[DEMO_MARK] !== true)
        return null;
      return {
        address: verified,
        addresses: (user.email_addresses ?? []).flatMap((held) =>
          signsIn(held) && held.email_address ? [held.email_address] : [],
        ),
        passwordSetAt: user.password_last_updated_at ?? null,
        secondFactor: user.two_factor_enabled === true,
      };
    },
    passwordMatches: async (userId, password) => {
      const response = await request(
        "POST",
        `/v1/users/${checked(userId, "user")}/verify_password`,
        { password },
        [400, 404, 422],
      );
      const answer = (await response?.json()) as
        { verified?: unknown } | undefined;
      return answer?.verified === true;
    },
    signInToken: async (userId, expiresInSeconds) => {
      const response = await request("POST", "/v1/sign_in_tokens", {
        user_id: checked(userId, "user"),
        expires_in_seconds: expiresInSeconds,
      });
      const made = (await response?.json()) as { token?: unknown } | undefined;
      if (typeof made?.token !== "string" || !made.token)
        throw new Error("Clerk made no sign-in token");
      return made.token;
    },
  };
}
