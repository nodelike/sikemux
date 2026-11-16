import type { BillingProductId, PlanFeature } from "@sikemux/protocol";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { applyProviderEvents } from "../src/billing/apply.ts";
import { StandInProvider } from "../src/billing/stand-in.ts";
import type { ProviderRequest } from "../src/billing/provider.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import { caller, emptyTables, migratedDatabase } from "./accounts.ts";
import { body, log, testApp } from "./support.ts";
import { sessionToken } from "./tokens.ts";

const PRO: PlanFeature[] = [
  "sync",
  "move_work",
  "history_search",
  "own_triggers",
];
const TEAM: PlanFeature[] = [
  ...PRO,
  "org_hosts",
  "org_shared_setup",
  "org_model_keys",
  "org_github",
  "org_bot_triggers",
  "chat_handover",
  "org_roles",
  "org_policy",
];

let database: Database;
let drop: () => Promise<void>;
let app: ReturnType<typeof testApp>;
let call: ReturnType<typeof caller>;
let delivered: ProviderRequest[];
let provider: StandInProvider;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

afterAll(() => drop());

function start(live: boolean) {
  provider = new StandInProvider("https://api.sikemux.com");
  app = testApp(database, new RateLimiter(), { billing: { provider, live } });
  call = caller(app);
  delivered = [];
  provider.deliverTo(async (request) => {
    delivered.push(request);
    await webhook(request);
  });
}

async function webhook(request: ProviderRequest) {
  const response = await app.request("/v1/billing/provider", {
    method: "POST",
    headers: {
      "x-stand-in-signature": request.header("x-stand-in-signature") ?? "",
    },
    body: request.body,
  });
  expect(response.status).toBe(204);
}

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs, billing_events cascade");
  start(true);
});

async function person(userId: string, { staff = true } = {}) {
  await database.db.insertInto("users").values({ id: userId }).execute();
  await database.db
    .updateTable("users")
    .set({ staff })
    .where("id", "=", userId)
    .execute();
  return sessionToken(userId);
}

async function org(ownerId: string, members: string[] = []) {
  const { id } = await database.db
    .insertInto("orgs")
    .values({ name: "Acme", created_by: ownerId })
    .returning("id")
    .executeTakeFirstOrThrow();
  await database.db
    .insertInto("org_users")
    .values([
      { org_id: id, user_id: ownerId, role: "owner", invited_by: null },
      ...members.map((user_id) => ({
        org_id: id,
        user_id,
        role: "user" as const,
        invited_by: ownerId,
      })),
    ])
    .execute();
  return id;
}

async function buy(
  token: string,
  product: BillingProductId,
  {
    quantity = 1,
    interval = "month",
    orgId,
  }: {
    quantity?: number;
    interval?: "month" | "year";
    orgId?: string;
  } = {},
) {
  const response = await call("/v1/billing/checkout", token, {
    method: "POST",
    json: { product, interval, quantity, ...(orgId ? { orgId } : {}) },
  });
  expect(response.status).toBe(200);
  const { url } = await body(response, "Checkout");
  const paid = await app.request(new URL(url).pathname);
  expect(paid.status).toBe(303);
}

async function overview(token: string, orgId?: string) {
  const response = await call(
    `/v1/billing${orgId ? `?org=${orgId}` : ""}`,
    token,
  );
  expect(response.status).toBe(200);
  return body(response, "BillingOverview");
}

const DAY_MS = 24 * 60 * 60_000;
const noCall = () =>
  Promise.reject(new Error("no call to the provider was expected"));

/** A Team plan of 3 seats, halfway through its month. */
async function halfwayTeam() {
  const owner = await person("user_owner");
  const orgId = await org("user_owner");
  await buy(owner, "team", { quantity: 3, orgId });
  await database.db
    .updateTable("subscriptions")
    .set({ current_period_end: new Date(Date.now() + 15 * DAY_MS) })
    .execute();
  const [subscription] = (await overview(owner, orgId)).subscriptions;
  const change = (json: object) =>
    call(`/v1/billing/subscriptions/${subscription?.id}`, owner, {
      method: "PATCH",
      json,
    });
  const paid = async () =>
    (
      await body(
        await call(`/v1/billing/invoices?org=${orgId}`, owner),
        "InvoiceList",
      )
    ).invoices;
  const held = async () => (await overview(owner, orgId)).subscriptions[0];
  return { change, paid, held };
}

async function invoices(token: string) {
  return (await body(await call("/v1/billing/invoices", token), "InvoiceList"))
    .invoices;
}

describe("what each plan allows", () => {
  it("allows Free nothing a plan pays for, and Pro its features", async () => {
    const ada = await person("user_ada");
    expect((await overview(ada)).allowance).toMatchObject({
      plan: "free",
      features: [],
      boxes: [],
    });
    await buy(ada, "pro");
    const { allowance, subscriptions } = await overview(ada);
    expect(allowance.plan).toBe("pro");
    expect(allowance.features.toSorted()).toEqual(PRO.toSorted());
    expect(subscriptions).toMatchObject([
      { product: "pro", quantity: 1, status: "active" },
    ]);
    const account = await body(await call("/v1/account", ada), "Account");
    expect(account.plan).toBe("pro");
  });

  it("shows the card a payer pays with and nothing for a payer without one", async () => {
    const ada = await person("user_ada");
    expect((await overview(ada)).card).toBeUndefined();
    await buy(ada, "pro");
    expect((await overview(ada)).card).toMatchObject({
      brand: "visa",
      last4: "4242",
    });
  });

  it("gives Team's features, Pro's among them, to everyone holding a seat", async () => {
    const owner = await person("user_owner");
    const member = await person("user_member", { staff: false });
    const orgId = await org("user_owner", ["user_member"]);
    await buy(owner, "team", { quantity: 3, orgId });

    const { allowance } = await overview(owner, orgId);
    expect(allowance.plan).toBe("team");
    expect(allowance.features.toSorted()).toEqual(TEAM.toSorted());

    const seat = (await overview(member)).allowance;
    expect(seat.plan).toBe("team");
    expect(seat.seatOrgs).toEqual([orgId]);
    expect(seat.features).toEqual(expect.arrayContaining(PRO));
    const account = await body(await call("/v1/account", member), "Account");
    expect(account.plan).toBe("team");
  });

  it("counts the Shed boxes bought, on any plan", async () => {
    const ada = await person("user_ada");
    await buy(ada, "shed_always_on", { quantity: 2, interval: "year" });
    const { allowance } = await overview(ada);
    expect(allowance.plan).toBe("free");
    expect(allowance.boxes).toEqual([
      { product: "shed_always_on", quantity: 2 },
    ]);
    expect((await invoices(ada))[0]?.total).toEqual({
      amount: 2 * 29_000,
      currency: "usd",
    });
  });

  it("allows everyone everything and answers only staff until billing is live", async () => {
    start(false);
    const ada = await person("user_ada");
    const { allowance, live } = await overview(ada);
    expect(live).toBe(false);
    expect(allowance.plan).toBe("free");
    expect(allowance.features.toSorted()).toEqual(TEAM.toSorted());
    const outsider = await person("user_out", { staff: false });
    expect((await call("/v1/billing", outsider)).status).toBe(404);
    expect((await call("/v1/billing/catalog", outsider)).status).toBe(404);
  });

  it("sells the Shed only to staff, even when live", async () => {
    const outsider = await person("user_out", { staff: false });
    const response = await call("/v1/billing/checkout", outsider, {
      method: "POST",
      json: { product: "shed", interval: "month", quantity: 1 },
    });
    expect(response.status).toBe(409);
  });
});

describe("refunds", () => {
  it("refunds a first payment in full and ends the plan", async () => {
    const ada = await person("user_ada");
    await buy(ada, "pro");
    const [first] = await invoices(ada);
    expect(first).toMatchObject({
      status: "paid",
      total: { amount: 800, currency: "usd" },
    });
    expect(first?.refundableUntil).toBeDefined();

    const response = await call(
      `/v1/billing/invoices/${first?.id}/refund`,
      ada,
      { method: "POST" },
    );
    expect(response.status).toBe(200);
    expect(await body(response, "Invoice")).toMatchObject({
      status: "refunded",
      total: { amount: 800 },
    });
    const { allowance, subscriptions } = await overview(ada);
    expect(allowance.plan).toBe("free");
    expect(subscriptions[0]?.status).toBe("ended");
  });

  it("waits for a provider that approves refunds later", async () => {
    provider.holdRefunds = true;
    const ada = await person("user_ada");
    await buy(ada, "pro");
    const [first] = await invoices(ada);
    const refund = () =>
      call(`/v1/billing/invoices/${first?.id}/refund`, ada, {
        method: "POST",
      });
    const asked = await body(await refund(), "Invoice");
    expect(asked.status).toBe("paid");
    expect(asked.refundRequestedAt).toBeDefined();
    expect(asked.refundableUntil).toBeUndefined();
    expect((await refund()).status).toBe(200);
    expect((await overview(ada)).allowance.plan).toBe("pro");

    await provider.approveRefunds();
    const [after] = await invoices(ada);
    expect(after).toMatchObject({ status: "refunded", total: { amount: 800 } });
    expect(after?.refundRequestedAt).toBeUndefined();
    expect((await overview(ada)).allowance.plan).toBe("free");
  });

  it("refuses a later payment, and a first one after 7 days", async () => {
    const ada = await person("user_ada");
    await buy(ada, "pro");
    const [subscription] = (await overview(ada)).subscriptions;
    const changed = await call(
      `/v1/billing/subscriptions/${subscription?.id}`,
      ada,
      { method: "PATCH", json: { interval: "year" } },
    );
    expect(changed.status).toBe(200);
    const [later, first] = (await invoices(ada)).map((invoice) => invoice.id);
    expect((await invoices(ada))[0]?.refundableUntil).toBeUndefined();
    const refund = (id: string) =>
      call(`/v1/billing/invoices/${id}/refund`, ada, { method: "POST" });
    expect((await refund(later ?? "")).status).toBe(409);

    await database.db
      .updateTable("invoices")
      .set({ at: new Date(Date.now() - 8 * 24 * 60 * 60_000) })
      .where("id", "=", first ?? "")
      .execute();
    expect((await refund(first ?? "")).status).toBe(409);
    expect((await overview(ada)).allowance.plan).toBe("pro");
  });

  it("refuses someone else's invoice as if it were not there", async () => {
    const ada = await person("user_ada");
    const bob = await person("user_bob");
    await buy(ada, "pro");
    const [invoice] = await invoices(ada);
    const response = await call(
      `/v1/billing/invoices/${invoice?.id}/refund`,
      bob,
      {
        method: "POST",
      },
    );
    expect(response.status).toBe(404);
    const receipt = await call(
      `/v1/billing/invoices/${invoice?.id}/receipt`,
      bob,
    );
    expect(receipt.status).toBe(404);
  });
});

describe("the provider's events", () => {
  it("changes nothing when an event comes again", async () => {
    const owner = await person("user_owner");
    const orgId = await org("user_owner");
    await buy(owner, "team", { quantity: 2, orgId });
    const [subscription] = (await overview(owner, orgId)).subscriptions;
    await call(`/v1/billing/subscriptions/${subscription?.id}`, owner, {
      method: "PATCH",
      json: { quantity: 5 },
    });
    const before = await database.pool.query(
      "select (select count(*) from invoices) as invoices, (select count(*) from events) as events, (select quantity from subscriptions) as quantity",
    );

    for (const request of delivered) await webhook(request);

    const after = await database.pool.query(
      "select (select count(*) from invoices) as invoices, (select count(*) from events) as events, (select quantity from subscriptions) as quantity",
    );
    expect(after.rows).toEqual(before.rows);
    expect(before.rows[0].quantity).toBe(5);
  });

  it("keeps a row for each product a provider holds under one subscription", async () => {
    await person("user_ada");
    const { id: customerRef } = await database.db
      .insertInto("billing_customers")
      .values({ user_id: "user_ada", provider: "boxes" })
      .returning("id")
      .executeTakeFirstOrThrow();
    const at = new Date();
    const subscription = {
      providerSubscriptionId: "sub_boxes",
      interval: "month" as const,
      currentPeriodEnd: new Date(at.getTime() + 30 * DAY_MS),
      cancelAtPeriodEnd: false,
    };
    const later = (minutes: number) =>
      new Date(at.getTime() + minutes * 60_000);
    await applyProviderEvents(
      database.db,
      { name: "boxes", cancel: noCall, refund: noCall },
      [
        {
          eventId: "evt_1",
          at,
          type: "subscription.started",
          customerRef,
          providerCustomerId: "cus_boxes",
          subscription: {
            ...subscription,
            items: [
              { product: "shed", quantity: 1 },
              { product: "shed_large", quantity: 2 },
            ],
          },
        },
        {
          eventId: "evt_2",
          at: later(1),
          type: "subscription.changed",
          subscription: {
            ...subscription,
            items: [{ product: "shed_large", quantity: 3 }],
          },
        },
        {
          eventId: "evt_3",
          at: later(2),
          type: "subscription.lapsed",
          providerSubscriptionId: "sub_boxes",
        },
      ],
      log,
    );
    const rows = await database.db
      .selectFrom("subscriptions")
      .select(["product", "quantity", "status"])
      .orderBy("product")
      .execute();
    expect(rows).toEqual([
      { product: "shed", quantity: 1, status: "ended" },
      { product: "shed_large", quantity: 3, status: "lapsed" },
    ]);
  });

  it("refuses an event that is not signed", async () => {
    const response = await app.request("/v1/billing/provider", {
      method: "POST",
      headers: { "x-stand-in-signature": "00" },
      body: "[]",
    });
    expect(response.status).toBe(401);
  });
});

describe("prices", () => {
  it("refuses a checkout that names its own amount", async () => {
    const ada = await person("user_ada");
    const response = await call("/v1/billing/checkout", ada, {
      method: "POST",
      json: { product: "pro", interval: "month", quantity: 1, amount: 1 },
    });
    expect(response.status).toBe(400);
  });

  it("charges the catalog's price for what was bought", async () => {
    const owner = await person("user_owner");
    const orgId = await org("user_owner");
    await buy(owner, "team", { quantity: 3, orgId });
    const response = await call(`/v1/billing/invoices?org=${orgId}`, owner);
    const { invoices: paid } = await body(response, "InvoiceList");
    expect(paid).toMatchObject([
      {
        lines: [{ product: "team", quantity: 3 }],
        reason: "first_payment",
        total: { amount: 6000, currency: "usd" },
      },
    ]);
  });

  it("charges for added seats only for the rest of the period", async () => {
    const { change, paid } = await halfwayTeam();
    expect(
      await body(await change({ quantity: 5 }), "Subscription"),
    ).toMatchObject({
      quantity: 5,
    });
    const [added] = await paid();
    expect(added).toMatchObject({
      lines: [{ product: "team", quantity: 5 }],
      reason: "change",
    });
    expect(added?.total.amount).toBeGreaterThan(1800);
    expect(added?.total.amount).toBeLessThan(2200);
  });

  it("changes nothing when the payment for a change fails", async () => {
    const { change, paid, held } = await halfwayTeam();
    provider.declineChanges = true;
    const response = await change({ quantity: 5 });
    expect(response.status).toBe(409);
    const [failed] = await paid();
    expect(failed).toMatchObject({ status: "failed", reason: "change" });
    expect((await held())?.quantity).toBe(3);
  });

  it("charges nothing for a change at renewal until the period ends", async () => {
    const { change, paid, held } = await halfwayTeam();
    provider.declineChanges = true;
    const response = await change({ quantity: 5, when: "renewal" });
    expect(await body(response, "Subscription")).toMatchObject({
      quantity: 3,
      atRenewal: { product: "team", interval: "month", quantity: 5 },
    });
    expect(await paid()).toHaveLength(1);
    expect((await held())?.quantity).toBe(3);
  });

  it("lets only the org's owner buy for it", async () => {
    await person("user_owner");
    const member = await person("user_member");
    const orgId = await org("user_owner", ["user_member"]);
    const response = await call("/v1/billing/checkout", member, {
      method: "POST",
      json: { product: "team", interval: "month", quantity: 2, orgId },
    });
    expect(response.status).toBe(403);
  });
});
