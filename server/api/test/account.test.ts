import { LiveCloseCode } from "@sikemux/protocol";
import {
  afterAll,
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";

import type { AppleSignIn } from "../src/account/apple.ts";
import { clerkBackend } from "../src/account/clerk.ts";
import { sweepClerk } from "../src/account/clerk-sweeper.ts";
import { purgeAccounts, pruneHistory } from "../src/account/purge.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import {
  caller,
  emptyTables,
  events,
  migratedDatabase,
  newDevice,
  registered,
  registration,
} from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { hostPeer, runApi, webPeer, type Running } from "./live.ts";
import { body, log, testApp } from "./support.ts";
import { macToken, sessionToken } from "./tokens.ts";

let database: Database;
let drop: () => Promise<void>;
let clerk: FakeClerk;
let apple: FakeApple;
let app: ReturnType<typeof testApp>;
let call: ReturnType<typeof caller>;
let api: Running | undefined;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

beforeEach(async () => {
  await emptyTables(database);
  clerk = new FakeClerk();
  apple = new FakeApple(clerk);
  app = testApp(database, new RateLimiter(), { clerk, appleSignIn: apple });
  call = caller(app);
});

afterEach(async () => {
  await api?.stop();
  api = undefined;
  vi.restoreAllMocks();
});

afterAll(() => drop());

/** Apple's token endpoints, recording what was revoked and whether Clerk had deleted the user yet. */
class FakeApple implements AppleSignIn {
  readonly revoked: {
    code?: string;
    token?: string;
    clientId: string;
    clerkDeleted: boolean;
  }[] = [];
  failing = false;

  private readonly clerk: FakeClerk;

  constructor(clerk: FakeClerk) {
    this.clerk = clerk;
  }

  async revokeCode(code: string, clientId: string) {
    if (this.failing) throw new Error("Apple answered 400 to /auth/token");
    this.revoked.push({
      code,
      clientId,
      clerkDeleted: this.clerk.deleted.length > 0,
    });
  }

  async revokeAccessToken(token: string, clientId: string) {
    if (this.failing) throw new Error("Apple answered 400 to /auth/revoke");
    this.revoked.push({
      token,
      clientId,
      clerkDeleted: this.clerk.deleted.length > 0,
    });
  }
}

/** A session token that verified its first factor `minutes` ago. */
function verified(userId: string, minutes = 1, claims = {}) {
  return sessionToken(userId, { claims: { fva: [minutes, -1], ...claims } });
}

async function deleteAccount(userId: string, token?: string, json?: unknown) {
  return call("/v1/account", token ?? (await verified(userId)), {
    method: "DELETE",
    json,
  });
}

async function accountActions() {
  const { rows } = await database.pool.query<{ action: string }>(
    "select action from audit where action like 'account.%' order by id",
  );
  return rows.map((row) => row.action);
}

async function accountWithDevices(userId: string) {
  const mac = await registered(app, userId, "host");
  const iphone = await registered(app, userId, "client", {
    sessionId: `sess_${userId}`,
  });
  return { mac, iphone };
}

/** Moves past the days a deletion can be restored in, and lets the sweeper delete in Clerk. */
async function restoreWindowEnds() {
  await database.pool.query(
    "update users set clerk_retry_at = now() where deleted_at is not null",
  );
  await sweepClerk(database.db, clerk, log);
}

async function count(sql: string, params: unknown[] = []) {
  const { rows } = await database.pool.query<{ n: number }>(sql, params);
  return rows[0]?.n;
}

describe("deleting an account", () => {
  it("revokes every device, tells the account, and deletes the user in Clerk after 30 days", async () => {
    const { mac, iphone } = await accountWithDevices("user_a");
    const response = await deleteAccount("user_a");
    expect(response.status).toBe(202);
    const deletion = await body(response, "AccountDeletion");
    expect(deletion.status).toBe("deleting");
    const days =
      (Date.parse(deletion.deletesAt) - Date.parse(deletion.requestedAt)) /
      86_400_000;
    expect(days).toBe(30);
    await sweepClerk(database.db, clerk, log);
    expect(clerk.deleted).toEqual([]);
    await restoreWindowEnds();
    expect(clerk.deleted).toEqual(["user_a"]);

    expect(await count("select count(*)::int as n from devices")).toBe(0);
    const { rows } = await database.pool.query(
      "select key, reason from removed_devices order by key",
    );
    expect(rows).toEqual(
      [mac.key, iphone.key]
        .sort()
        .map((key) => ({ key, reason: "account_deleted" })),
    );
    const told = await events(database);
    expect(told.slice(-3)).toMatchObject([
      { type: "device.revoked", reason: "account_deleted" },
      { type: "device.revoked", reason: "account_deleted" },
      { type: "account.deleted", subject: null },
    ]);
    expect(await accountActions()).toEqual([
      "account.deletion_requested",
      "account.deleted_in_clerk",
    ]);
    expect(apple.revoked).toEqual([]);
  });

  it("refuses every token of the account afterwards, and never recreates it", async () => {
    await accountWithDevices("user_a");
    const session = await sessionToken("user_a");
    expect((await deleteAccount("user_a")).status).toBe(202);

    expect((await call("/v1/devices", session)).status).toBe(401);
    expect(
      (await call("/v1/devices/challenge", session, { method: "POST" })).status,
    ).toBe(401);
    const mac = await macToken("user_a");
    expect((await call("/v1/devices", mac)).status).toBe(401);
    const registering = await call("/v1/devices", mac, {
      method: "POST",
      json: await registration(app, newDevice(), "user_a", mac, {
        nonce: "a".repeat(64),
      }),
    });
    expect(registering.status).toBe(401);
    expect(
      await count(
        "select count(*)::int as n from users where id = 'user_a' and deleted_at is not null",
      ),
    ).toBe(1);
  });

  it("refuses the account's live connections: the web app's at once, a host's after its farewell", async () => {
    const { mac } = await accountWithDevices("user_a");
    await deleteAccount("user_a");
    api = await runApi(database);

    const web = await webPeer(api.url, "user_a");
    expect(await web.closed).toBe(LiveCloseCode.unauthenticated);
    const host = await hostPeer(api.url, mac);
    const revoked = await host.until("revoked");
    expect(revoked.reason).toBe("account_deleted");
    expect(await host.closed).toBe(LiveCloseCode.revoked);
    expect(host.messages.some((message) => message.type === "ready")).toBe(
      false,
    );
  });

  it("closes the account's open connections with revoked", async () => {
    const { mac } = await accountWithDevices("user_a");
    api = await runApi(database);
    const host = await hostPeer(api.url, mac);
    await host.until("ready");
    await deleteAccount("user_a");
    const frame = await host.until("events");
    expect(frame.events.map((event) => event.type)).toContain(
      "account.deleted",
    );
    expect((await host.until("revoked")).reason).toBe("account_deleted");
  });

  it("writes one deletion however many requests race", async () => {
    await accountWithDevices("user_a");
    const token = await verified("user_a");
    const responses = await Promise.all(
      [1, 2, 3].map(() => deleteAccount("user_a", token)),
    );
    expect(responses.map((response) => response.status)).toEqual([
      202, 202, 202,
    ]);
    expect(
      await count(
        "select count(*)::int as n from events where type = 'account.deleted'",
      ),
    ).toBe(1);
    expect(
      await count(
        "select count(*)::int as n from audit where action = 'account.deletion_requested'",
      ),
    ).toBe(1);
  });

  it("retries when Clerk fails, and the sweeper finishes it", async () => {
    await accountWithDevices("user_a");
    clerk.failing = true;
    const response = await deleteAccount("user_a");
    expect((await body(response, "AccountDeletion")).status).toBe("deleting");
    await restoreWindowEnds();
    const { rows } = await database.pool.query(
      "select clerk_attempts, clerk_retry_at > now() as later from users where id = 'user_a'",
    );
    expect(rows).toEqual([{ clerk_attempts: 1, later: true }]);

    await sweepClerk(database.db, clerk, log);
    expect(clerk.deleted).toEqual([]);

    clerk.failing = false;
    await database.pool.query("update users set clerk_retry_at = now()");
    await sweepClerk(database.db, clerk, log);
    expect(clerk.deleted).toEqual(["user_a"]);
    expect(
      await count(
        "select count(*)::int as n from users where clerk_deleted_at is not null",
      ),
    ).toBe(1);

    const again = await deleteAccount("user_a");
    expect((await body(again, "AccountDeletion")).status).toBe("deleted");
  });

  it("backs off, and alerts after six failed attempts", async () => {
    await accountWithDevices("user_a");
    clerk.failing = true;
    await deleteAccount("user_a");
    const error = vi.spyOn(log, "error");
    for (let attempt = 1; attempt <= 6; attempt++) {
      await database.pool.query("update users set clerk_retry_at = now()");
      await sweepClerk(database.db, clerk, log);
    }
    const { rows } = await database.pool.query(
      "select clerk_attempts, extract(epoch from clerk_retry_at - now())::int as wait from users",
    );
    expect(rows[0].clerk_attempts).toBe(6);
    expect(rows[0].wait).toBeGreaterThan(30 * 60);
    expect(rows[0].wait).toBeLessThanOrEqual(60 * 60);
    expect(error).toHaveBeenCalledOnce();
  });

  it("still deletes here when the API has no Clerk secret, leaving Clerk pending", async () => {
    app = testApp(database, new RateLimiter(), { clerk: null });
    call = caller(app);
    await accountWithDevices("user_a");
    const response = await deleteAccount("user_a");
    expect(response.status).toBe(202);
    expect((await body(response, "AccountDeletion")).status).toBe("deleting");
    expect(await count("select count(*)::int as n from devices")).toBe(0);
  });

  it("needs a session verified in the last ten minutes", async () => {
    await accountWithDevices("user_a");
    for (const token of [
      await verified("user_a", 11),
      await sessionToken("user_a"),
    ]) {
      const response = await deleteAccount("user_a", token);
      expect(response.status).toBe(403);
      const { error } = await body(response, "ApiError");
      expect(error.code).toBe("reverify_required");
      expect(error.message).toBe("reverify");
    }
    expect(await count("select count(*)::int as n from devices")).toBe(2);
  });

  it("cannot be done with the Mac app's token", async () => {
    await accountWithDevices("user_a");
    const response = await deleteAccount("user_a", await macToken("user_a"));
    expect(response.status).toBe(403);
    expect(await count("select count(*)::int as n from devices")).toBe(2);
  });

  it("works for an account that never registered a device", async () => {
    const response = await deleteAccount("user_new");
    expect(response.status).toBe(202);
    expect(
      (await call("/v1/devices", await sessionToken("user_new"))).status,
    ).toBe(401);
  });
});

describe("revoking Sign in with Apple when an account is deleted", () => {
  it("revokes the token the phone's code trades for, as the phone app", async () => {
    const response = await deleteAccount("user_a", undefined, {
      appleAuthorizationCode: "c0de",
    });
    expect(response.status).toBe(202);
    expect(apple.revoked).toEqual([
      {
        code: "c0de",
        clientId: "com.nodelike.sikemux.mobile",
        clerkDeleted: false,
      },
    ]);
    expect(clerk.deleted).toEqual([]);
    expect(await accountActions()).toEqual([
      "account.deletion_requested",
      "account.apple_revoked",
    ]);
  });

  it("revokes the token Clerk holds from a web sign-in before Clerk deletes the user", async () => {
    clerk.appleTokens.set("user_a", ["at_1"]);
    await deleteAccount("user_a");
    expect(apple.revoked).toEqual([
      {
        token: "at_1",
        clientId: "com.nodelike.sikemux.signin",
        clerkDeleted: false,
      },
    ]);
  });

  it("uses the dev app's bundle id on a dev API, which has no web sign-in with Apple", async () => {
    app = testApp(database, new RateLimiter(), {
      clerk,
      appleSignIn: apple,
      push: { app: "dev", allowSandbox: true },
    });
    call = caller(app);
    clerk.appleTokens.set("user_a", ["at_1"]);
    clerk.appleTokens.set("user_b", ["at_2"]);
    await deleteAccount("user_a", undefined, {
      appleAuthorizationCode: "c0de",
    });
    await deleteAccount("user_b");
    expect(apple.revoked).toEqual([
      {
        code: "c0de",
        clientId: "com.nodelike.sikemux.mobile.dev",
        clerkDeleted: false,
      },
    ]);
  });

  it("deletes the account even when Apple refuses", async () => {
    await accountWithDevices("user_a");
    apple.failing = true;
    const response = await deleteAccount("user_a", undefined, {
      appleAuthorizationCode: "c0de",
    });
    expect(response.status).toBe(202);
    expect(await count("select count(*)::int as n from devices")).toBe(0);
    expect(await accountActions()).toEqual([
      "account.deletion_requested",
      "account.apple_revoke_failed",
    ]);
  });

  it("deletes the account when the API cannot revoke Sign in with Apple", async () => {
    app = testApp(database, new RateLimiter(), { clerk, appleSignIn: null });
    call = caller(app);
    const response = await deleteAccount("user_a", undefined, {
      appleAuthorizationCode: "c0de",
    });
    expect(response.status).toBe(202);
    await restoreWindowEnds();
    expect(clerk.deleted).toEqual(["user_a"]);
  });

  it("refuses a body that is not a deletion request, and deletes nothing", async () => {
    await accountWithDevices("user_a");
    for (const json of [
      { appleAuthorizationCode: "c".repeat(513) },
      { appleAuthorizationCode: "" },
      { other: true },
    ]) {
      const response = await deleteAccount("user_a", undefined, json);
      expect(response.status).toBe(400);
      expect((await body(response, "ApiError")).error.code).toBe("bad_request");
    }
    expect(await count("select count(*)::int as n from devices")).toBe(2);
    expect(apple.revoked).toEqual([]);
  });

  it("asks for reverification before it reads the body", async () => {
    const response = await deleteAccount(
      "user_a",
      await verified("user_a", 11),
      { appleAuthorizationCode: "c".repeat(513) },
    );
    expect(response.status).toBe(403);
    expect((await body(response, "ApiError")).error.code).toBe(
      "reverify_required",
    );
  });
});

describe("removing a phone from elsewhere", () => {
  it("revokes the session that registered it", async () => {
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(clerk.revoked).toEqual(["sess_phone"]);
  });

  it("leaves the session alone when the phone signs itself out", async () => {
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_phone" } }),
      { method: "DELETE" },
    );
    expect(clerk.revoked).toEqual([]);
  });

  it("removes the phone even when Clerk fails, and retries the revocation", async () => {
    const phone = await registered(app, "user_a", "client", {
      sessionId: "sess_phone",
    });
    clerk.failing = true;
    const response = await call(
      `/v1/devices/${phone.key}`,
      await sessionToken("user_a", { claims: { sid: "sess_web" } }),
      { method: "DELETE" },
    );
    expect(response.status).toBe(204);
    clerk.failing = false;
    await database.pool.query(
      "update removed_devices set clerk_retry_at = now()",
    );
    await sweepClerk(database.db, clerk, log);
    expect(clerk.revoked).toEqual(["sess_phone"]);
    await sweepClerk(database.db, clerk, log);
    expect(clerk.revoked).toEqual(["sess_phone"]);
  });
});

describe("purging", () => {
  it("erases every trace of the account after 30 days, keeping only a count", async () => {
    await accountWithDevices("user_a");
    await accountWithDevices("user_b");
    await deleteAccount("user_a");

    expect(await purgeAccounts(database.db, log)).toBe(0);
    await restoreWindowEnds();
    await database.pool.query(
      "update users set purge_after = now() - interval '1 second' where id = 'user_a'",
    );
    expect(await purgeAccounts(database.db, log)).toBe(1);

    const { rows: columns } = await database.pool.query<{
      table_name: string;
      column_name: string;
    }>(
      "select table_name, column_name from information_schema.columns where table_schema = 'public' and table_name <> 'schema_migrations'",
    );
    const holding: string[] = [];
    for (const { table_name, column_name } of columns) {
      const found = await count(
        `select count(*)::int as n from "${table_name}" where "${column_name}"::text like '%user_a%'`,
      );
      if (found) holding.push(`${table_name}.${column_name}`);
    }
    expect(holding).toEqual([]);
    const { rows } = await database.pool.query(
      "select user_id, actor, detail from audit where action = 'account.purged'",
    );
    expect(rows).toEqual([
      { user_id: null, actor: "system", detail: { devices: 2 } },
    ]);
    expect(await count("select count(*)::int as n from devices")).toBe(2);
  });

  it("waits for Clerk before purging", async () => {
    await accountWithDevices("user_a");
    clerk.failing = true;
    await deleteAccount("user_a");
    await database.pool.query(
      "update users set purge_after = now() - interval '1 second'",
    );
    expect(await purgeAccounts(database.db, log)).toBe(0);
  });

  it("prunes old history and remembers how far, so returning devices reset", async () => {
    const { mac, iphone } = await accountWithDevices("user_a");
    await call(`/v1/devices/${iphone.key}`, await macToken("user_a"), {
      method: "DELETE",
    });
    await database.pool.query(
      "update events set at = now() - interval '401 days' where subject = $1 and type = 'device.added'",
      [iphone.key],
    );
    await database.pool.query(
      "update removed_devices set removed_at = now() - interval '31 days'",
    );
    await database.pool.query(
      "insert into audit (user_id, actor, action, at) values ('user_a', 'system', 'old', now() - interval '401 days')",
    );
    await database.pool.query(
      "insert into challenges (nonce, user_id, expires_at) values ('n', 'user_a', now() - interval '2 hours')",
    );

    const pruned = await pruneHistory(database.db, log);
    expect(pruned).toEqual({
      events: 1,
      removedDevices: 1,
      audit: 1,
      shares: 0,
      challenges: 1,
    });
    const { rows } = await database.pool.query(
      "select events_pruned_through::int as through from users",
    );
    const added = (await events(database)).find(
      (event) => event.subject === mac.key,
    );
    expect(rows[0].through).toBeGreaterThan(added?.id ?? 0);
  });
});

describe("the Clerk Backend API client", () => {
  function stub(status: number, answer: unknown = {}) {
    const requests: { url: string; method: string; auth: string | null }[] = [];
    const fetcher = async (
      input: string | URL | Request,
      init?: RequestInit,
    ) => {
      requests.push({
        url: String(input),
        method: init?.method ?? "GET",
        auth: new Headers(init?.headers).get("authorization"),
      });
      return new Response(JSON.stringify(answer), { status });
    };
    return { requests, fetcher };
  }

  it("deletes users and revokes sessions with the secret key", async () => {
    const { requests, fetcher } = stub(200);
    const client = clerkBackend("sk_test_abc", fetcher);
    await client.deleteUser("user_a");
    await client.revokeSession("sess_1");
    expect(requests).toEqual([
      {
        url: "https://api.clerk.com/v1/users/user_a",
        method: "DELETE",
        auth: "Bearer sk_test_abc",
      },
      {
        url: "https://api.clerk.com/v1/sessions/sess_1/revoke",
        method: "POST",
        auth: "Bearer sk_test_abc",
      },
    ]);
  });

  it("counts a user or session Clerk no longer has as done", async () => {
    const client = clerkBackend("sk_test_abc", stub(404).fetcher);
    await expect(client.deleteUser("user_a")).resolves.toBeUndefined();
    await expect(client.revokeSession("sess_1")).resolves.toBeUndefined();
  });

  it("fails on anything else, so the sweeper retries", async () => {
    const client = clerkBackend("sk_test_abc", stub(500).fetcher);
    await expect(client.deleteUser("user_a")).rejects.toThrow("500");
  });

  it("reads the Apple access tokens Clerk holds, and none for a user it no longer has", async () => {
    const { requests, fetcher } = stub(200, [
      { object: "oauth_access_token", provider: "oauth_apple", token: "at_1" },
      { object: "oauth_access_token", provider: "oauth_apple", token: "" },
    ]);
    expect(
      await clerkBackend("sk_test_abc", fetcher).appleAccessTokens("user_a"),
    ).toEqual(["at_1"]);
    expect(requests).toEqual([
      {
        url: "https://api.clerk.com/v1/users/user_a/oauth_access_tokens/oauth_apple",
        method: "GET",
        auth: "Bearer sk_test_abc",
      },
    ]);
    expect(
      await clerkBackend("sk_test_abc", stub(404).fetcher).appleAccessTokens(
        "user_a",
      ),
    ).toEqual([]);
  });

  it("never puts an id that is not Clerk's into the URL", async () => {
    const { requests, fetcher } = stub(200);
    const client = clerkBackend("sk_test_abc", fetcher);
    await expect(client.deleteUser("../sessions")).rejects.toThrow();
    expect(requests).toEqual([]);
  });
});
