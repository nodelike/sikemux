import { createHmac, randomUUID } from "node:crypto";
import type { BillingInterval, BillingProductId } from "@sikemux/protocol";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { PRODUCTS } from "../src/billing/catalog.ts";
import { PaddleProvider } from "../src/billing/paddle.ts";
import {
  CLIENT_TOKEN_NAME,
  type PaddleApi,
  type PaddlePrice,
} from "../src/billing/paddle-api.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import { caller, emptyTables, migratedDatabase } from "./accounts.ts";
import { FakeClerk } from "./clerk.ts";
import { APP_ORIGIN, body, log, testApp } from "./support.ts";
import { sessionToken } from "./tokens.ts";

const SECRET = "pdl_ntfset_review_secret";
const WEBHOOK = "https://api.sikemux.com/v1/billing/provider";
const MINUTE_MS = 60_000;
const DAY_MS = 24 * 60 * MINUTE_MS;

const OWNER = "user_owner";
const OTHER = "user_b";
const NOT_STAFF = "user_public";

const ago = (minutes: number) => new Date(Date.now() - minutes * MINUTE_MS);

interface PaddleSub {
  id: string;
  status: string;
  customer_id: string;
  custom_data: Record<string, unknown> | null;
  billing_cycle: { interval: string; frequency: number };
  current_billing_period: { starts_at: string; ends_at: string };
  next_billed_at: string;
  scheduled_change: { action: string; effective_at: string } | null;
  items: { status: string; quantity: number; price: PaddlePrice }[];
  management_urls: { update_payment_method: string };
}

interface Call {
  method: string;
  path: string;
  body?: Record<string, unknown>;
}

interface Patch {
  heldByPaddleBefore: number;
  asked: number;
  mode: unknown;
  heldUntil: string | undefined;
}

/** Paddle as its docs describe it, for the calls the adapter makes. */
class FakePaddle {
  readonly sandbox = true;
  setUp = true;
  calls: Call[] = [];
  patches: Patch[] = [];
  subscriptions = new Map<string, PaddleSub>();
  completed = new Map<string, unknown[]>();
  prices: PaddlePrice[] = PRODUCTS.flatMap((item) =>
    item.prices.map(({ interval, unit }) => ({
      id: `pri_${item.id}_${interval}`,
      product_id: `pro_${item.id}`,
      status: "active" as const,
      billing_cycle: { interval, frequency: 1 },
      unit_price: {
        amount: String(unit.amount),
        currency_code: unit.currency.toUpperCase(),
      },
      quantity: { minimum: 1, maximum: item.maxQuantity },
      custom_data: { product: item.id, interval },
    })),
  );

  price(product: BillingProductId, interval: BillingInterval = "month") {
    const found = this.prices.find(
      (price) => price.id === `pri_${product}_${interval}`,
    );
    if (!found) throw new Error(`no price for ${product} each ${interval}`);
    return found;
  }

  list(path: string): Promise<unknown[]> {
    this.calls.push({ method: "LIST", path });
    if (path.startsWith("/prices")) return Promise.resolve(this.prices);
    if (path.startsWith("/notification-settings"))
      return Promise.resolve(
        this.setUp
          ? [
              {
                destination: WEBHOOK,
                active: true,
                endpoint_secret_key: SECRET,
              },
            ]
          : [],
      );
    if (path.startsWith("/client-tokens"))
      return Promise.resolve(
        this.setUp
          ? [
              {
                name: CLIENT_TOKEN_NAME,
                status: "active",
                token: "test_public",
              },
            ]
          : [],
      );
    return Promise.reject(new Error(`the fake Paddle lists no ${path}`));
  }

  request(
    method: string,
    path: string,
    body?: Record<string, unknown>,
  ): Promise<unknown> {
    this.calls.push({ method, path, ...(body ? { body } : {}) });
    if (method === "POST" && path === "/transactions") {
      const id = `txn_${randomUUID()}`;
      return Promise.resolve({
        id,
        checkout: { url: `${APP_ORIGIN}/pay?_ptxn=${id}` },
      });
    }
    const paid = /^\/transactions\?subscription_id=([^&]+)/.exec(path);
    if (method === "GET" && paid)
      return Promise.resolve(
        (this.completed.get(paid[1] ?? "") ?? []).slice(-1),
      );
    const subscription = /^\/subscriptions\/([^/?]+)$/.exec(path);
    const sub = subscription && this.subscriptions.get(subscription[1] ?? "");
    if (sub && method === "GET") return Promise.resolve(sub);
    if (sub && method === "PATCH" && body) {
      const items = body.items as
        { price_id: string; quantity: number }[] | undefined;
      const [first] = items ?? [];
      if (first) {
        this.patches.push({
          heldByPaddleBefore: sub.items[0]?.quantity ?? 0,
          asked: first.quantity,
          mode: body.proration_billing_mode,
          heldUntil: (
            body.custom_data as { held?: { until: string } } | undefined
          )?.held?.until,
        });
        const price = this.prices.find((each) => each.id === first.price_id);
        if (!price) throw new Error(`no price ${first.price_id}`);
        sub.items = [{ status: "active", quantity: first.quantity, price }];
        sub.billing_cycle = {
          interval: price.billing_cycle?.interval ?? "month",
          frequency: 1,
        };
      }
      if ("custom_data" in body)
        sub.custom_data = body.custom_data as Record<string, unknown>;
      if (body.scheduled_change === null) sub.scheduled_change = null;
      return Promise.resolve(sub);
    }
    if (method === "POST" && path === "/adjustments")
      return Promise.resolve({});
    if (method === "POST" && path.endsWith("/cancel"))
      return Promise.resolve({});
    return Promise.reject(
      new Error(`the fake Paddle has no ${method} ${path}`),
    );
  }
}

let database: Database;
let drop: () => Promise<void>;
let paddle: FakePaddle;
let app: ReturnType<typeof testApp>;
let call: ReturnType<typeof caller>;

function appWith(fake: FakePaddle, live = true) {
  return testApp(database, new RateLimiter(), {
    clerk: new FakeClerk(),
    billing: {
      provider: new PaddleProvider({
        api: fake as unknown as PaddleApi,
        apiOrigin: "https://api.sikemux.com",
        log,
      }),
      live,
    },
  });
}

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

afterAll(() => drop());

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs, billing_events cascade");
  await database.pool.query(
    "insert into users (id, staff) values ($1, true), ($2, true), ($3, false)",
    [OWNER, OTHER, NOT_STAFF],
  );
  paddle = new FakePaddle();
  app = appWith(paddle);
  call = caller(app);
});

async function as(
  userId: string,
  path: string,
  init: { method?: string; json?: unknown } = {},
) {
  return call(path, await sessionToken(userId), init);
}

function signed(text: string, { secret = SECRET, at = Date.now() } = {}) {
  const ts = Math.floor(at / 1000);
  const h1 = createHmac("sha256", secret).update(`${ts}:${text}`).digest("hex");
  return `ts=${ts};h1=${h1}`;
}

function eventText(type: string, data: unknown, at: Date, id = randomUUID()) {
  return JSON.stringify({
    event_id: `evt_${id}`,
    event_type: type,
    occurred_at: at.toISOString(),
    data,
  });
}

/** One webhook as Paddle sends it; answers the status the API gave. */
async function deliver(type: string, data: unknown, at = new Date()) {
  const text = eventText(type, data, at);
  const response = await app.request("/v1/billing/provider", {
    method: "POST",
    headers: { "paddle-signature": signed(text) },
    body: text,
  });
  return response.status;
}

function paddleSub(
  customerId: string,
  customerRef: string | undefined,
  product: BillingProductId,
  quantity: number,
  ends = new Date(Date.now() + 29 * DAY_MS).toISOString(),
): PaddleSub {
  const id = `sub_${randomUUID()}`;
  return {
    id,
    management_urls: {
      update_payment_method: `https://portal.paddle.test/${id}?signed_in_as=${customerId}`,
    },
    status: "active",
    customer_id: customerId,
    custom_data:
      customerRef === undefined ? null : { customer_ref: customerRef },
    billing_cycle: { interval: "month", frequency: 1 },
    current_billing_period: {
      starts_at: ago(DAY_MS / MINUTE_MS).toISOString(),
      ends_at: ends,
    },
    next_billed_at: ends,
    scheduled_change: null,
    items: [{ status: "active", quantity, price: paddle.price(product) }],
  };
}

function paddleTxn(sub: PaddleSub, origin: string, last4 = "0000") {
  const [item] = sub.items;
  const txn = {
    id: `txn_${randomUUID()}`,
    origin,
    customer_id: sub.customer_id,
    subscription_id: sub.id,
    custom_data: sub.custom_data,
    currency_code: "USD",
    billing_period: sub.current_billing_period,
    items: sub.items.map(({ quantity, price }) => ({ quantity, price })),
    details: {
      totals: {
        grand_total: String(
          Number(item?.price.unit_price.amount) * (item?.quantity ?? 0),
        ),
      },
    },
    payments: [
      {
        status: "captured",
        method_details: {
          type: "card",
          card: { type: "visa", last4, expiry_month: 1, expiry_year: 2031 },
        },
      },
    ],
    checkout: { url: null },
  };
  paddle.completed.set(sub.id, [...(paddle.completed.get(sub.id) ?? []), txn]);
  return txn;
}

/** The checkout the API opens at Paddle: what it sent, and the customer it names as ours. */
async function openCheckout(
  userId: string,
  product: BillingProductId,
  quantity = 1,
  orgId?: string,
) {
  const response = await as(userId, "/v1/billing/checkout", {
    method: "POST",
    json: { product, interval: "month", quantity, ...(orgId ? { orgId } : {}) },
  });
  const opened = paddle.calls.findLast(
    (each) => each.method === "POST" && each.path === "/transactions",
  );
  const custom = opened?.body?.custom_data as
    { customer_ref: string } | undefined;
  return {
    status: response.status,
    sent: opened?.body,
    ref: custom?.customer_ref ?? "",
  };
}

/**
 * A checkout opened through the API and paid at Paddle: Paddle makes the subscription under the
 * customer whose email was typed, and sends the completed payment. Answers both.
 */
async function pay(
  userId: string,
  product: BillingProductId,
  {
    quantity = 1,
    orgId,
    paddleCustomer = `ctm_${userId}`,
    at = ago(60),
    ends,
    last4,
  }: {
    quantity?: number;
    orgId?: string;
    paddleCustomer?: string;
    at?: Date;
    ends?: string;
    last4?: string;
  } = {},
) {
  const { status, sent, ref } = await openCheckout(
    userId,
    product,
    quantity,
    orgId,
  );
  expect(status).toBe(200);
  const sub = paddleSub(paddleCustomer, ref, product, quantity, ends);
  paddle.subscriptions.set(sub.id, sub);
  const txn = paddleTxn(sub, "api", last4);
  expect(await deliver("transaction.completed", txn, at)).toBe(204);
  return { sub, txn, opened: sent, ref };
}

async function overview(userId: string, orgId?: string) {
  return body(
    await as(userId, `/v1/billing${orgId ? `?org=${orgId}` : ""}`),
    "BillingOverview",
  );
}

async function createOrg() {
  const response = await as(OWNER, "/v1/orgs", {
    method: "POST",
    json: { name: "Acme" },
  });
  expect(response.status).toBe(201);
  return (await body(response, "Org")).id;
}

async function change(userId: string, id: string | undefined, json: unknown) {
  return as(userId, `/v1/billing/subscriptions/${id}`, {
    method: "PATCH",
    json,
  });
}

async function team(seats: number, options: { ends?: string } = {}) {
  const orgId = await createOrg();
  const { sub, txn } = await pay(OWNER, "team", {
    quantity: seats,
    orgId,
    ...options,
  });
  const [held] = (await overview(OWNER, orgId)).subscriptions;
  return { orgId, sub, txn, id: held?.id };
}

function chargeback(transactionId: string, total: number) {
  return {
    id: `adj_${randomUUID()}`,
    action: "chargeback",
    type: "full",
    status: "approved",
    transaction_id: transactionId,
    currency_code: "USD",
    totals: { total: String(total) },
  };
}

async function invoices(userId: string) {
  return (await body(await as(userId, "/v1/billing/invoices"), "InvoiceList"))
    .invoices;
}

const prorated = () =>
  paddle.patches.filter((each) => each.mode === "prorated_immediately");

describe("a change held for renewal, then a change now", () => {
  it("F1a: a change now while one is held is refused, so nothing is prorated from unpaid seats", async () => {
    const { sub, id } = await team(3);
    expect(
      (await change(OWNER, id, { quantity: 1000, when: "renewal" })).status,
    ).toBe(200);
    expect(await deliver("subscription.updated", sub, ago(30))).toBe(204);

    expect((await change(OWNER, id, { quantity: 4 })).status).toBe(409);
    expect(
      (
        await as(OWNER, `/v1/billing/subscriptions/${id}/preview`, {
          method: "POST",
          json: { quantity: 4 },
        })
      ).status,
    ).toBe(409);
    expect(prorated()).toEqual([]);
  });

  it("F1b: 5 seats paid and 50 held for renewal: 50 now is refused and 5 stay held", async () => {
    const { orgId, sub, id } = await team(5);
    expect(
      (await change(OWNER, id, { quantity: 50, when: "renewal" })).status,
    ).toBe(200);
    expect(await deliver("subscription.updated", sub, ago(30))).toBe(204);

    expect((await change(OWNER, id, { quantity: 50 })).status).toBe(409);

    const [held] = (await overview(OWNER, orgId)).subscriptions;
    expect({
      seats: [held?.quantity, held?.atRenewal?.quantity],
      prorated: prorated(),
    }).toEqual({ seats: [5, 50], prorated: [] });
  });
});

describe("taking back a change held for renewal", () => {
  it("F3: asking for what is paid gives Paddle the paid items back, unbilled", async () => {
    const { orgId, sub, id } = await team(5);
    expect(
      (await change(OWNER, id, { quantity: 50, when: "renewal" })).status,
    ).toBe(200);
    expect(await deliver("subscription.updated", sub, ago(30))).toBe(204);
    const [pending] = (await overview(OWNER, orgId)).subscriptions;
    expect([pending?.quantity, pending?.atRenewal?.quantity]).toEqual([5, 50]);

    const asked = paddle.patches.length;
    const back = await change(OWNER, id, { quantity: 5 });
    expect(await deliver("subscription.updated", sub, ago(20))).toBe(204);
    const again = await change(OWNER, id, { quantity: 5, when: "renewal" });

    const [after] = (await overview(OWNER, orgId)).subscriptions;
    expect({
      answers: [back.status, again.status],
      askedOfPaddle: paddle.patches.slice(asked).map((each) => each.mode),
      paddleBillsAtRenewal: sub.items[0]?.quantity,
      held: sub.custom_data?.held,
      shownAtRenewal: after?.atRenewal?.quantity,
    }).toEqual({
      answers: [200, 200],
      askedOfPaddle: ["do_not_bill"],
      paddleBillsAtRenewal: 5,
      held: undefined,
      shownAtRenewal: undefined,
    });
  });
});

describe("a change at renewal asked while our period is behind Paddle's", () => {
  it("F9: the paid seats are held until Paddle's period end", async () => {
    const ended = ago(1).toISOString();
    const { orgId, sub, id } = await team(5, { ends: ended });
    const ends = new Date(Date.now() + 30 * DAY_MS).toISOString();
    sub.current_billing_period = { starts_at: ended, ends_at: ends };
    sub.next_billed_at = ends;

    expect(
      (await change(OWNER, id, { quantity: 50, when: "renewal" })).status,
    ).toBe(200);
    expect(await deliver("subscription.updated", sub)).toBe(204);

    const last = paddle.patches.at(-1);
    const [held] = (await overview(OWNER, orgId)).subscriptions;
    expect({
      mode: last?.mode,
      oldSeatsHeldUntil: last?.heldUntil,
      seats: [held?.quantity, held?.atRenewal?.quantity],
    }).toEqual({
      mode: "do_not_bill",
      oldSeatsHeldUntil: ends,
      seats: [5, 50],
    });
  });
});

describe("the webhook", () => {
  it("F11a: an item the setup did not make leaves the rest of the subscription heard", async () => {
    const { sub } = await pay(OWNER, "pro");
    const byHand: PaddlePrice = {
      ...paddle.price("pro"),
      id: "pri_made_in_the_dashboard",
      billing_cycle: null,
      custom_data: null,
    };
    const charged: PaddleSub = {
      ...sub,
      scheduled_change: {
        action: "cancel",
        effective_at: sub.current_billing_period.ends_at,
      },
      items: [...sub.items, { status: "active", quantity: 1, price: byHand }],
    };
    const answer = await deliver("subscription.updated", charged, ago(10));
    const [held] = (await overview(OWNER)).subscriptions;
    expect({ answer, cancelAtPeriodEnd: held?.cancelAtPeriodEnd }).toEqual({
      answer: 204,
      cancelAtPeriodEnd: true,
    });
  });

  it("F11b: before setup, strangers' requests ask Paddle at most once a minute", async () => {
    paddle.setUp = false;
    for (let sent = 0; sent < 5; sent++) {
      await app.request("/v1/billing/provider", {
        method: "POST",
        headers: { "paddle-signature": "ts=1;h1=00" },
        body: "{}",
      });
      await app.request("/v1/billing/paddle");
    }
    const asked = (start: string) =>
      paddle.calls.filter((each) => each.path.startsWith(start)).length;
    expect({
      settings: asked("/notification-settings"),
      tokens: asked("/client-tokens"),
    }).toEqual({ settings: 1, tokens: 1 });
  });
});

describe("a chargeback", () => {
  it("F6: ends the plan now, here and with Paddle, and the invoice reads disputed", async () => {
    const { txn } = await pay(OWNER, "pro");
    const answer = await deliver(
      "adjustment.created",
      chargeback(txn.id, 800),
      ago(20),
    );
    const [invoice] = await invoices(OWNER);
    const after = await overview(OWNER);
    expect({
      answer,
      invoice: invoice?.status,
      plan: after.allowance.plan,
      cancelsAskedOfPaddle: paddle.calls.filter(
        (each) => each.method === "POST" && each.path.endsWith("/cancel"),
      ).length,
    }).toEqual({
      answer: 204,
      invoice: "disputed",
      plan: "free",
      cancelsAskedOfPaddle: 1,
    });
  });
});
