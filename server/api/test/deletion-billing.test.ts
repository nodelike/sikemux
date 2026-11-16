import { createHmac, randomBytes } from "node:crypto";
import type { BillingProductId } from "@sikemux/protocol";
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

import { sweepClerk } from "../src/account/clerk-sweeper.ts";
import { purgeAccounts } from "../src/account/purge.ts";
import { applyProviderEvents } from "../src/billing/apply.ts";
import { sweepDeletedBilling } from "../src/billing/deleted.ts";
import type { ProviderRequest } from "../src/billing/provider.ts";
import { StandInProvider } from "../src/billing/stand-in.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import { caller, emptyTables, migratedDatabase } from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { body, log, testApp } from "./support.ts";
import { sessionToken } from "./tokens.ts";

const SECRET_BYTES = randomBytes(24);
const SECRET = `whsec_${SECRET_BYTES.toString("base64")}`;
const DAY_MS = 24 * 60 * 60_000;

const OWNER = "user_owner";
const ADMIN = "user_admin";
const MEMBER = "user_member";

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let call: ReturnType<typeof caller>;
let provider: StandInProvider;
let clerk: FakeClerk;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

afterAll(() => drop());

afterEach(() => {
  vi.useRealTimers();
});

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs, billing_events cascade");
  await database.pool.query(
    "insert into users (id, staff) select unnest($1::text[]), true",
    [[OWNER, ADMIN, MEMBER]],
  );
  provider = new StandInProvider("https://api.sikemux.com");
  clerk = new FakeClerk();
  app = testApp(database, new RateLimiter(), {
    clerk,
    webhookSecret: SECRET,
    billing: { provider, live: true },
  });
  call = caller(app);
  provider.deliverTo(async (request) => {
    expect((await providerWebhook(request)).status).toBe(204);
  });
});

function providerWebhook(request: ProviderRequest) {
  return app.request("/v1/billing/provider", {
    method: "POST",
    headers: {
      "x-stand-in-signature": request.header("x-stand-in-signature") ?? "",
    },
    body: request.body,
  });
}

async function as(
  userId: string,
  path: string,
  init: { method?: string; json?: unknown } = {},
) {
  return call(path, await sessionToken(userId), init);
}

async function deleteAccount(userId: string) {
  const token = await sessionToken(userId, { claims: { fva: [0, -1] } });
  return call("/v1/account", token, { method: "DELETE" });
}

function clerkDeleted(userId: string) {
  const id = `msg_${randomBytes(8).toString("hex")}`;
  const timestamp = Math.floor(Date.now() / 1000);
  const payload = JSON.stringify({
    type: "user.deleted",
    object: "event",
    data: { id: userId, object: "user", deleted: true },
  });
  const signature = createHmac("sha256", SECRET_BYTES)
    .update(`${id}.${timestamp}.${payload}`)
    .digest("base64");
  return app.request("/v1/webhooks/clerk", {
    method: "POST",
    body: payload,
    headers: {
      "content-type": "application/json",
      "svix-id": id,
      "svix-timestamp": String(timestamp),
      "svix-signature": `v1,${signature}`,
    },
  });
}

async function checkout(
  userId: string,
  product: BillingProductId,
  { quantity = 1, orgId }: { quantity?: number; orgId?: string } = {},
) {
  const response = await as(userId, "/v1/billing/checkout", {
    method: "POST",
    json: {
      product,
      interval: "month",
      quantity,
      ...(orgId ? { orgId } : {}),
    },
  });
  expect(response.status).toBe(200);
  const { url } = await body(response, "Checkout");
  return () => app.request(new URL(url).pathname);
}

async function buy(
  userId: string,
  product: BillingProductId,
  options: { quantity?: number; orgId?: string } = {},
) {
  const pay = await checkout(userId, product, options);
  expect((await pay()).status).toBe(303);
}

async function createOrg(seats: number) {
  const response = await as(OWNER, "/v1/orgs", {
    method: "POST",
    json: { name: "Acme" },
  });
  expect(response.status).toBe(201);
  const { id } = await body(response, "Org");
  await buy(OWNER, "team", { quantity: seats, orgId: id });
  return id;
}

async function invite(orgId: string, role: "admin" | "user") {
  const response = await as(OWNER, `/v1/orgs/${orgId}/invites`, {
    method: "POST",
    json: { role },
  });
  expect(response.status).toBe(201);
  return body(response, "OrgInviteCreated");
}

async function addMember(
  orgId: string,
  userId: string,
  role: "admin" | "user",
) {
  const { token } = await invite(orgId, role);
  const joined = await as(userId, "/v1/orgs/join", {
    method: "POST",
    json: { token },
  });
  expect(joined.status).toBe(200);
}

async function subscriptions() {
  const { rows } = await database.pool.query<{
    product: string;
    status: string;
    cancel_at_period_end: boolean;
    payer: string;
  }>(
    `select s.product, s.status, s.cancel_at_period_end, coalesce(c.user_id, c.org_id::text) as payer
       from subscriptions s join billing_customers c on c.id = s.customer_id
      order by s.started_at, s.id`,
  );
  return rows;
}

async function members(orgId: string) {
  const { rows } = await database.pool.query<{ user_id: string; role: string }>(
    "select user_id, role from org_users where org_id = $1 order by joined_at, user_id",
    [orgId],
  );
  return rows;
}

async function count(query: string, params: unknown[] = []) {
  const { rows } = await database.pool.query<{ n: number }>(query, params);
  return rows[0]?.n;
}

/** Moves the account past the days its deletion can be restored in, deleted in Clerk too. */
async function dueForPurge(userId: string) {
  await database.pool.query(
    "update users set clerk_retry_at = now() where id = $1",
    [userId],
  );
  await sweepClerk(database.db, clerk, log);
  await database.pool.query(
    "update users set purge_after = now() - interval '1 second', billing_retry_at = now() where id = $1",
    [userId],
  );
}

describe("deleting an account", () => {
  it("sets the person's own plan to end at the period's end", async () => {
    await buy(MEMBER, "pro");
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    expect(await subscriptions()).toEqual([
      {
        product: "pro",
        status: "active",
        cancel_at_period_end: true,
        payer: MEMBER,
      },
    ]);
  });

  it("ends a plan paid for on a checkout page left open while the account was deleted", async () => {
    const pay = await checkout(MEMBER, "pro");
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    expect((await pay()).status).toBe(303);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(false);

    await sweepDeletedBilling(database.db, provider, log);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(true);
  });

  it("ends a box paid for late when the account's plan was already set to end", async () => {
    await buy(MEMBER, "pro");
    const pay = await checkout(MEMBER, "shed");
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    expect((await pay()).status).toBe(303);

    await sweepDeletedBilling(database.db, provider, log);
    expect(
      (await subscriptions()).map((row) => [
        row.product,
        row.cancel_at_period_end,
      ]),
    ).toEqual([
      ["pro", true],
      ["shed", true],
    ]);
  });

  it("cancels a lapsed plan at once, so the provider's retry of its payment charges nobody", async () => {
    await buy(MEMBER, "pro");
    const { rows } = await database.pool.query<{ id: string }>(
      "select provider_subscription_id as id from subscriptions",
    );
    const providerSubscriptionId = rows[0]?.id ?? "";
    const failedAt = new Date(Date.now() + 60_000);
    const payment = {
      providerSubscriptionId,
      lines: [{ product: "pro" as const, quantity: 1 }],
      reason: "renewal" as const,
      total: { amount: 800, currency: "usd" as const },
    };
    await applyProviderEvents(
      database.db,
      provider,
      [
        {
          eventId: "evt_failed",
          at: failedAt,
          type: "payment.failed",
          providerPaymentId: "pay_try_1",
          ...payment,
        },
        {
          eventId: "evt_lapsed",
          at: failedAt,
          type: "subscription.lapsed",
          providerSubscriptionId,
        },
      ],
      log,
    );
    const cancel = vi.spyOn(provider, "cancel");
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    expect(cancel.mock.calls.map(([, when]) => when)).toEqual(["now"]);
    expect((await subscriptions())[0]?.status).toBe("ended");
  });

  it("leaves an org someone else owns at once, freeing the seat", async () => {
    const orgId = await createOrg(2);
    await addMember(orgId, MEMBER, "user");
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    expect(await members(orgId)).toEqual([{ user_id: OWNER, role: "owner" }]);
    await addMember(orgId, ADMIN, "user");
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(false);
  });

  it("keeps an org the person is alone in until the purge, its plan ending and invites revoked", async () => {
    const orgId = await createOrg(3);
    const { token } = await invite(orgId, "user");
    expect((await deleteAccount(OWNER)).status).toBe(202);
    expect(await members(orgId)).toEqual([{ user_id: OWNER, role: "owner" }]);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(true);
    const joined = await as(MEMBER, "/v1/orgs/join", {
      method: "POST",
      json: { token },
    });
    expect(joined.status).toBe(404);
  });

  it("refuses an org's only owner while others are in it, and lets them go once someone else owns it", async () => {
    const orgId = await createOrg(3);
    await addMember(orgId, ADMIN, "admin");
    expect((await deleteAccount(OWNER)).status).toBe(409);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(false);

    const { rows } = await database.pool.query<{ id: string }>(
      "select id from org_users where org_id = $1 and user_id = $2",
      [orgId, ADMIN],
    );
    const promoted = await as(
      OWNER,
      `/v1/orgs/${orgId}/members/${rows[0]?.id}/role`,
      { method: "PUT", json: { role: "owner" } },
    );
    expect(promoted.status).toBe(200);
    expect((await deleteAccount(OWNER)).status).toBe(202);
    expect(await members(orgId)).toEqual([{ user_id: ADMIN, role: "owner" }]);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(true);
  });

  it("passes the org to its longest-standing admin when Clerk deletes its only owner", async () => {
    const orgId = await createOrg(4);
    await addMember(orgId, MEMBER, "user");
    await addMember(orgId, ADMIN, "admin");
    await database.pool.query("delete from events");

    expect((await clerkDeleted(OWNER)).status).toBe(204);
    expect(await members(orgId)).toEqual([
      { user_id: MEMBER, role: "user" },
      { user_id: ADMIN, role: "owner" },
    ]);
    expect(
      await count(
        "select count(*)::int as n from events where type = 'plan.changed' and user_id = $1",
        [ADMIN],
      ),
    ).toBe(1);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(true);
    const { rows } = await database.pool.query(
      "select user_id, action, subject, detail from audit where action like 'org.%' and detail->>'why' = 'account_deleted' order by id",
    );
    expect(rows).toEqual([
      {
        user_id: ADMIN,
        action: "org.role_changed",
        subject: orgId,
        detail: { from: "admin", to: "owner", why: "account_deleted" },
      },
      {
        user_id: OWNER,
        action: "org.left",
        subject: orgId,
        detail: { role: "owner", why: "account_deleted" },
      },
    ]);
  });

  it("stops charging an owner whose sign-in was deleted for the org that passed on", async () => {
    const orgId = await createOrg(2);
    await addMember(orgId, ADMIN, "admin");
    expect((await clerkDeleted(OWNER)).status).toBe(204);
    const paid = async () =>
      (
        await body(
          await as(ADMIN, `/v1/billing/invoices?org=${orgId}`),
          "InvoiceList",
        )
      ).invoices.length;
    const before = await paid();

    const heir = await body(
      await as(ADMIN, `/v1/billing?org=${orgId}`),
      "BillingOverview",
    );
    const [held] = heir.subscriptions;
    const more = await as(ADMIN, `/v1/billing/subscriptions/${held?.id}`, {
      method: "PATCH",
      json: { quantity: 50 },
    });
    expect({
      card: heir.card,
      cancelAtPeriodEnd: held?.cancelAtPeriodEnd,
      addsSeats: more.status,
      charged: (await paid()) - before,
    }).toEqual({
      card: undefined,
      cancelAtPeriodEnd: true,
      addsSeats: 409,
      charged: 0,
    });
  });

  it("passes the org to its longest-standing member when it has no admin", async () => {
    const orgId = await createOrg(4);
    await addMember(orgId, MEMBER, "user");
    await addMember(orgId, ADMIN, "user");
    expect((await clerkDeleted(OWNER)).status).toBe(204);
    expect(await members(orgId)).toEqual([
      { user_id: MEMBER, role: "owner" },
      { user_id: ADMIN, role: "user" },
    ]);
  });

  it("tries the provider again later, backing off, while it cannot be reached", async () => {
    await buy(MEMBER, "pro");
    provider.refuseCancels = true;
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    const retry = async () =>
      (
        await database.pool.query<{ attempts: number; wait: number }>(
          "select billing_attempts as attempts, extract(epoch from billing_retry_at - now())::int as wait from users where id = $1",
          [MEMBER],
        )
      ).rows[0];
    const first = await retry();
    expect(first?.attempts).toBe(1);

    await database.pool.query(
      "update users set billing_retry_at = now() where id = $1",
      [MEMBER],
    );
    await sweepDeletedBilling(database.db, provider, log);
    const second = await retry();
    expect(second?.attempts).toBe(2);
    expect(second?.wait).toBeGreaterThan(first?.wait ?? 0);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(false);

    provider.refuseCancels = false;
    await database.pool.query(
      "update users set billing_retry_at = now() where id = $1",
      [MEMBER],
    );
    await sweepDeletedBilling(database.db, provider, log);
    expect((await retry())?.attempts).toBe(0);
    expect((await subscriptions())[0]?.cancel_at_period_end).toBe(true);
  });
});

describe("an account deleted while it joins an org", () => {
  async function joinWhileDeleting(orgId: string, id: string) {
    await database.pool.query(
      "insert into users (id, staff) values ($1, true)",
      [id],
    );
    const { token } = await invite(orgId, "user");
    const [joined, deleted] = await Promise.all([
      as(id, "/v1/orgs/join", { method: "POST", json: { token } }),
      deleteAccount(id),
    ]);
    const { rows } = await database.pool.query(
      `select 1 from org_users m join users u on u.id = m.user_id
        where m.user_id = $1 and u.deleted_at is not null`,
      [id],
    );
    return {
      joined: joined.status,
      deleted: deleted.status,
      leftInOrg: rows.length > 0,
    };
  }

  it("is never left in the org, and is erased when due", async () => {
    const orgId = await createOrg(30);
    const rounds = [];
    for (let round = 0; round < 10; round += 1)
      rounds.push(await joinWhileDeleting(orgId, `user_racer${round}`));
    expect(rounds.filter((round) => round.leftInOrg)).toEqual([]);
    expect(new Set(rounds.map((round) => round.deleted))).toEqual(
      new Set([202]),
    );
    for (const round of rounds) expect([200, 401]).toContain(round.joined);

    await dueForPurge("user_racer0");
    await sweepDeletedBilling(database.db, provider, log);
    await purgeAccounts(database.db, log);
    expect(
      await count("select count(*)::int as n from users where id = $1", [
        "user_racer0",
      ]),
    ).toBe(0);
    expect((await subscriptions()).map((row) => row.status)).toEqual([
      "active",
    ]);
  });

  it("is no reason to end the org's plan, nor keeps the account from being erased", async () => {
    const orgId = await createOrg(5);
    await addMember(orgId, MEMBER, "user");
    await database.pool.query(
      "insert into users (id, staff, deleted_at, purge_after) values ('user_racer', true, now(), now() + interval '30 days')",
    );
    await database.pool.query(
      "insert into org_users (org_id, user_id, role) values ($1, 'user_racer', 'user')",
      [orgId],
    );
    await dueForPurge("user_racer");
    await sweepDeletedBilling(database.db, provider, log);
    expect(await purgeAccounts(database.db, log)).toBe(1);
    expect(await subscriptions()).toMatchObject([
      { status: "active", cancel_at_period_end: false },
    ]);
  });
});

describe("restoring a deleted account", () => {
  it("lets the person resume their plan, without their orgs", async () => {
    await buy(MEMBER, "pro");
    const orgId = await createOrg(3);
    await addMember(orgId, MEMBER, "user");
    expect((await deleteAccount(MEMBER)).status).toBe(202);
    expect(
      (await as(MEMBER, "/v1/account/restore", { method: "POST" })).status,
    ).toBe(200);

    const { subscriptions: held } = await body(
      await as(MEMBER, "/v1/billing"),
      "BillingOverview",
    );
    const pro = held.find((subscription) => subscription.product === "pro");
    expect(pro?.cancelAtPeriodEnd).toBe(true);
    const resumed = await as(
      MEMBER,
      `/v1/billing/subscriptions/${pro?.id}/resume`,
      { method: "POST" },
    );
    expect(resumed.status).toBe(200);
    expect((await body(resumed, "Subscription")).cancelAtPeriodEnd).toBe(false);
    expect(await members(orgId)).toEqual([{ user_id: OWNER, role: "owner" }]);
  });
});

describe("purging a deleted account", () => {
  it("waits for the provider to end what is still running, then erases it with the org it was alone in", async () => {
    await buy(OWNER, "pro");
    const orgId = await createOrg(3);
    expect((await deleteAccount(OWNER)).status).toBe(202);
    await dueForPurge(OWNER);

    provider.refuseCancels = true;
    await sweepDeletedBilling(database.db, provider, log);
    expect(await purgeAccounts(database.db, log)).toBe(0);
    expect(
      await count("select count(*)::int as n from users where id = $1", [
        OWNER,
      ]),
    ).toBe(1);

    provider.refuseCancels = false;
    await database.pool.query(
      "update users set billing_retry_at = now() where id = $1",
      [OWNER],
    );
    await sweepDeletedBilling(database.db, provider, log);
    expect((await subscriptions()).map((row) => row.status)).toEqual([
      "ended",
      "ended",
    ]);
    expect(await purgeAccounts(database.db, log)).toBe(1);
    expect(
      await count("select count(*)::int as n from users where id = $1", [
        OWNER,
      ]),
    ).toBe(0);
    expect(
      await count("select count(*)::int as n from orgs where id = $1", [orgId]),
    ).toBe(0);
    expect(await count("select count(*)::int as n from invoices")).toBe(0);
    expect(await count("select count(*)::int as n from subscriptions")).toBe(0);
  });
});

describe("the provider's events", () => {
  async function erasedSubscription() {
    await buy(MEMBER, "pro");
    const { subscriptions: held } = await body(
      await as(MEMBER, "/v1/billing"),
      "BillingOverview",
    );
    const id = held[0]?.id;
    const row = await database.db
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .selectAll("subscriptions")
      .select("billing_customers.provider_customer_id")
      .where("subscriptions.id", "=", id ?? "")
      .executeTakeFirstOrThrow();
    await database.pool.query("delete from users where id = $1", [MEMBER]);
    return row;
  }

  async function lateEnd(sentAgo: number) {
    const row = await erasedSubscription();
    const captured: ProviderRequest[] = [];
    provider.deliverTo(async (request) => {
      captured.push(request);
    });
    vi.useFakeTimers({ toFake: ["Date"], now: Date.now() - sentAgo });
    await provider.cancel(
      {
        providerSubscriptionId: row.provider_subscription_id,
        providerCustomerId: row.provider_customer_id ?? "",
        product: row.product,
        interval: row.interval,
        quantity: row.quantity,
        currentPeriodEnd: row.current_period_end,
        cancelAtPeriodEnd: row.cancel_at_period_end,
      },
      "now",
    );
    vi.useRealTimers();
    const [request] = captured;
    if (!request) throw new Error("The stand-in sent nothing.");
    return providerWebhook(request);
  }

  it("acknowledges an event about a subscription that was erased", async () => {
    expect((await lateEnd(provider.eventRetrySpanMs + DAY_MS)).status).toBe(
      204,
    );
  });

  it("asks again for a fresh event about a subscription it does not know yet", async () => {
    expect((await lateEnd(0)).status).toBe(503);
  });

  it("ends and refunds a second plan that a checkout race let through", async () => {
    const first = await checkout(MEMBER, "pro");
    const second = await checkout(MEMBER, "pro");
    expect((await first()).status).toBe(303);
    expect((await second()).status).toBe(303);

    expect(
      (await subscriptions()).map(({ status, cancel_at_period_end }) => ({
        status,
        cancel_at_period_end,
      })),
    ).toEqual([
      { status: "active", cancel_at_period_end: false },
      { status: "ended", cancel_at_period_end: false },
    ]);
    const { rows } = await database.pool.query<{ status: string }>(
      "select status from invoices order by at, id",
    );
    expect(rows.map((invoice) => invoice.status)).toEqual(["paid", "refunded"]);
  });
});
