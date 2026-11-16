import { createHmac, randomUUID } from "node:crypto";
import type { BillingInterval, BillingProductId } from "@sikemux/protocol";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { PRODUCTS } from "../src/billing/catalog.ts";
import { PaddleProvider } from "../src/billing/paddle.ts";
import { endSandboxPlans } from "../src/billing/sandbox.ts";
import {
  CLIENT_TOKEN_NAME,
  PaddleError,
  type PaddleApi,
  type PaddlePrice,
} from "../src/billing/paddle-api.ts";
import type { Database } from "../src/db.ts";
import { RateLimiter } from "../src/limits.ts";
import { seatLimit } from "../src/billing/seats.ts";
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

/** The Paddle customer a person's verified email finds or makes. */
const ctmOf = (userId: string) => `ctm_${userId.replaceAll("_", "")}`;

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
  sandbox = true;
  setUp = true;
  calls: Call[] = [];
  patches: Patch[] = [];
  /** How far Paddle's clock moves on between an items change and the next call. */
  gapAfterItems = 0;
  private later = 0;
  /** How long Paddle takes to answer for a subscription. */
  slowMs = 0;
  subscriptions = new Map<string, PaddleSub>();
  customerOf = new Map<string, string>();
  lastCheckout: string | undefined;
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
    if (method === "POST" && path === "/customers") {
      const email = String(body?.email);
      const held = this.customerOf.get(email);
      if (held)
        return Promise.reject(
          new PaddleError(
            409,
            "customer_already_exists",
            `Customer email conflicts with existing customer ID ${held}`,
          ),
        );
      const first = ctmOf(email.replace("verified address of ", ""));
      const id = [...this.customerOf.values()].includes(first)
        ? `ctm_made${this.customerOf.size}`
        : first;
      this.customerOf.set(email, id);
      return Promise.resolve({ id });
    }
    if (method === "PATCH" && path.startsWith("/customers/")) {
      const id = path.slice("/customers/".length);
      if (typeof body?.email === "string") {
        const taken = this.customerOf.get(body.email);
        if (taken && taken !== id)
          return Promise.reject(
            new PaddleError(
              409,
              "customer_already_exists",
              `Customer email conflicts with existing customer ID ${taken}`,
            ),
          );
        for (const [email, held] of this.customerOf)
          if (held === id) this.customerOf.delete(email);
        this.customerOf.set(body.email, id);
      }
      return Promise.resolve({});
    }
    if (method === "POST" && path === "/transactions") {
      const id = `txn_${randomUUID()}`;
      this.lastCheckout = id;
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
    if (sub && method === "GET") {
      const { slowMs } = this;
      return slowMs
        ? new Promise((resolve) => setTimeout(() => resolve(sub), slowMs))
        : Promise.resolve(sub);
    }
    if (sub && method === "PATCH" && body) {
      const items = body.items as
        { price_id: string; quantity: number }[] | undefined;
      const [first] = items ?? [];
      const billsOn = body.next_billed_at as string | undefined;
      const now = Date.now() + ahead + this.later;
      const renewsIn = new Date(sub.next_billed_at).getTime() - now;
      if (renewsIn > 0 && renewsIn < 30 * MINUTE_MS)
        return Promise.reject(
          new PaddleError(409, "subscription_locked_renewal", "locked"),
        );
      if (billsOn && new Date(billsOn).getTime() - now < 30 * MINUTE_MS)
        return Promise.reject(
          new PaddleError(
            409,
            "subscription_next_billed_at_too_soon",
            "New `next_billed_at` must be at least 30 minutes from now.",
          ),
        );
      if (first && billsOn && billsOn !== sub.next_billed_at)
        return Promise.reject(
          new PaddleError(
            400,
            "bad_request",
            "not possible to change items and billing date in the same request",
          ),
        );
      if (billsOn && !first) {
        sub.next_billed_at = billsOn;
        sub.current_billing_period.ends_at = billsOn;
      }
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
        const interval = price.billing_cycle?.interval ?? "month";
        if (interval !== sub.billing_cycle.interval) {
          // Paddle starts a new period at a change of interval, as the sandbox showed.
          const ends = new Date(
            Date.now() + (interval === "year" ? 365 : 30) * DAY_MS,
          ).toISOString();
          sub.current_billing_period = {
            starts_at: new Date().toISOString(),
            ends_at: ends,
          };
          sub.next_billed_at = ends;
        }
        sub.billing_cycle = { interval, frequency: 1 };
        this.later += this.gapAfterItems;
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
let clerk: MovingClerk;

/** Clerk, where a verified address can pass from one account to another. */
class MovingClerk extends FakeClerk {
  readonly addressOf = new Map<string, string | null>();

  override async verifiedEmail(userId: string) {
    const address = this.addressOf.get(userId);
    return address === null ? null : `verified address of ${address ?? userId}`;
  }
}

function appWith(fake: FakePaddle, live = true) {
  return testApp(database, new RateLimiter(), {
    clerk,
    billing: {
      provider: new PaddleProvider({
        api: fake as unknown as PaddleApi,
        apiOrigin: "https://api.sikemux.com",
        log,
        now: () => Date.now() + ahead,
      }),
      live,
    },
  });
}

/** How far the adapter's clock and Paddle's run ahead of the test's, to stand past a period's end. */
let ahead = 0;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

afterAll(() => drop());

beforeEach(async () => {
  ahead = 0;
  await emptyTables(database);
  await database.pool.query("truncate orgs, billing_events cascade");
  await database.pool.query(
    "insert into users (id, staff) values ($1, true), ($2, true), ($3, false)",
    [OWNER, OTHER, NOT_STAFF],
  );
  paddle = new FakePaddle();
  clerk = new MovingClerk();
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

function signed(
  text: string,
  { secret = SECRET, at = Date.now() + ahead } = {},
) {
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
    custom_data: null,
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

function paddleTxn(
  sub: PaddleSub,
  { id = `txn_${randomUUID()}`, last4 = "0000" } = {},
) {
  const [item] = sub.items;
  const txn = {
    id,
    origin: "api",
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

/** The checkout the API opens at Paddle: what it sent, and Paddle's id for it. */
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
  return {
    status: response.status,
    sent: opened?.body,
    txn: response.status === 200 ? paddle.lastCheckout : undefined,
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
    paddleCustomer = ctmOf(userId),
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
  const {
    status,
    sent,
    txn: id,
  } = await openCheckout(userId, product, quantity, orgId);
  expect(status).toBe(200);
  const sub = paddleSub(paddleCustomer, product, quantity, ends);
  paddle.subscriptions.set(sub.id, sub);
  const txn = paddleTxn(sub, { id, last4 });
  expect(await deliver("transaction.completed", txn, at)).toBe(204);
  return { sub, txn, opened: sent };
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

/** Paddle has renewed: the next period is open and the clock stands a minute past the paid one's end. */
function renewed(sub: PaddleSub, paidEnd: Date) {
  ahead = paidEnd.getTime() - Date.now() + MINUTE_MS;
  const ends = new Date(paidEnd.getTime() + 30 * DAY_MS).toISOString();
  sub.current_billing_period = {
    starts_at: paidEnd.toISOString(),
    ends_at: ends,
  };
  sub.next_billed_at = ends;
  return structuredClone(sub);
}

const justAfter = (paidEnd: Date) => new Date(paidEnd.getTime() + 1000);
const clockNow = () => new Date(Date.now() + ahead);

/** A Team plan whose change for renewal Paddle holds and the API has heard of. */
async function teamHolding(paid: number, atRenewal: number) {
  const paidEnd = new Date(Date.now() + 40 * MINUTE_MS);
  const held = await team(paid, { ends: paidEnd.toISOString() });
  const asked = await change(OWNER, held.id, {
    quantity: atRenewal,
    when: "renewal",
  });
  expect(asked.status).toBe(200);
  expect(await deliver("subscription.updated", held.sub)).toBe(204);
  return { ...held, paidEnd };
}

const heldAtPaddle = (sub: PaddleSub) =>
  (sub.custom_data as { held?: { quantity: number } } | null)?.held?.quantity;

/** Paddle takes the next change of a subscription's items, and its answer never reaches the API. */
function loseNextAnswer() {
  const request = paddle.request.bind(paddle);
  let lost = false;
  paddle.request = async (method, path, body) => {
    const answer = await request(method, path, body);
    if (lost || method !== "PATCH" || !body?.items) return answer;
    lost = true;
    throw new Error("The operation was aborted due to timeout");
  };
}

/** How many of nine invites the org's owner gets made. */
async function invitesMade(orgId: string) {
  let made = 0;
  for (let each = 0; each < 9; each++) {
    const invited = await as(OWNER, `/v1/orgs/${orgId}/invites`, {
      method: "POST",
      json: { role: "user" },
    });
    if (invited.status === 201) made++;
  }
  return made;
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

async function paidBy(orgId: string) {
  const { rows } = await database.pool.query<{ paid_by: string | null }>(
    "select paid_by from billing_customers where org_id = $1",
    [orgId],
  );
  return rows[0]?.paid_by ?? null;
}

const askedOfPaddle = (ending: string) =>
  paddle.calls.filter(
    (each) => each.method === "POST" && each.path.endsWith(ending),
  ).length;

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
  it("F3b: a box added now after the take-back keeps a change of interval now refused until renewal", async () => {
    await pay(OWNER, "shed", { quantity: 2 });
    const [box] = (await overview(OWNER)).subscriptions;
    await change(OWNER, box?.id, { interval: "year", when: "renewal" });
    await change(OWNER, box?.id, { interval: "month" });
    const more = await change(OWNER, box?.id, { quantity: 3 });
    const asked = paddle.patches.length;
    const yearly = await change(OWNER, box?.id, { interval: "year" });
    expect({
      answers: [more.status, yearly.status],
      askedOfPaddle: paddle.patches.length - asked,
    }).toEqual({ answers: [200, 409], askedOfPaddle: 0 });
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

  it("R4: a subscription made by hand in Paddle is acknowledged when it misses a payment or ends", async () => {
    const byHand: PaddlePrice = {
      ...paddle.price("pro"),
      id: "pri_made_in_the_dashboard",
      billing_cycle: null,
      custom_data: null,
    };
    const sub: PaddleSub = {
      ...paddleSub("ctm_ofsomeoneelse", "pro", 1),
      items: [{ status: "active", quantity: 1, price: byHand }],
    };
    expect({
      active: await deliver("subscription.updated", sub, ago(9)),
      pastDue: await deliver(
        "subscription.past_due",
        { ...sub, status: "past_due" },
        ago(6),
      ),
      canceled: await deliver(
        "subscription.canceled",
        { ...sub, status: "canceled" },
        ago(3),
      ),
    }).toEqual({ active: 204, pastDue: 204, canceled: 204 });
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

describe("a cured lapse", () => {
  const renewal = (sub: PaddleSub) => ({
    ...paddleTxn(sub),
    origin: "subscription_recurring",
  });

  async function lapsed() {
    const { sub } = await pay(OWNER, "pro");
    expect(
      await deliver(
        "subscription.past_due",
        { ...sub, status: "past_due" },
        ago(30),
      ),
    ).toBe(204);
    expect((await overview(OWNER)).subscriptions[0]?.status).toBe("lapsed");
    const ends = new Date(Date.now() + 59 * DAY_MS).toISOString();
    const renewed: PaddleSub = {
      ...sub,
      current_billing_period: {
        starts_at: sub.current_billing_period.ends_at,
        ends_at: ends,
      },
      next_billed_at: ends,
    };
    return { renewed, ends };
  }

  it("S4: the subscription stamped first and sent first", async () => {
    const { renewed, ends } = await lapsed();
    expect(await deliver("subscription.updated", renewed, ago(11))).toBe(204);
    expect(
      await deliver("transaction.completed", renewal(renewed), ago(10)),
    ).toBe(204);
    const [held] = (await overview(OWNER)).subscriptions;
    expect([held?.status, held?.currentPeriodEnd]).toEqual(["active", ends]);
  });

  it("F7a: the subscription stamped after the payment, and sent first", async () => {
    const { renewed, ends } = await lapsed();
    expect(await deliver("subscription.updated", renewed, ago(10))).toBe(204);
    expect(
      await deliver("transaction.completed", renewal(renewed), ago(11)),
    ).toBe(204);
    const [held] = (await overview(OWNER)).subscriptions;
    expect([held?.status, held?.currentPeriodEnd]).toEqual(["active", ends]);
  });

  it("F7b: the subscription stamped before the payment, and sent second", async () => {
    const { renewed, ends } = await lapsed();
    expect(
      await deliver("transaction.completed", renewal(renewed), ago(10)),
    ).toBe(204);
    expect(await deliver("subscription.updated", renewed, ago(11))).toBe(204);
    const [held] = (await overview(OWNER)).subscriptions;
    expect([held?.status, held?.currentPeriodEnd]).toEqual(["active", ends]);
  });
});

describe("who pays for an org", () => {
  it("F2: of two owners' open checkouts, the one paid makes its opener the payer", async () => {
    const orgId = await createOrg();
    await database.pool.query(
      "insert into org_users (org_id, user_id, role) values ($1, $2, 'owner')",
      [orgId, OTHER],
    );
    const first = await openCheckout(OWNER, "team", 3, orgId);
    const second = await openCheckout(OTHER, "team", 3, orgId);

    const sub = paddleSub(ctmOf(OWNER), "team", 3);
    paddle.subscriptions.set(sub.id, sub);
    expect(
      await deliver(
        "transaction.completed",
        paddleTxn(sub, { id: first.txn }),
        ago(5),
      ),
    ).toBe(204);

    const payer = await paidBy(orgId);
    const [held] = (await overview(OTHER, orgId)).subscriptions;
    const raised = await change(OTHER, held?.id, { quantity: 1000 });
    const portal = await as(OTHER, `/v1/billing/portal?org=${orgId}`);
    const left = await as(OWNER, `/v1/orgs/${orgId}/leave`, { method: "POST" });

    expect({
      checkouts: [first.status, second.status],
      payer,
      otherOwnerRaisesSeatsNow: raised.status,
      otherOwnerOpensThePayersPortal: portal.status,
      payerLeaves: left.status,
      cancelsAskedOfPaddle: askedOfPaddle("/cancel"),
    }).toEqual({
      checkouts: [200, 200],
      payer: OWNER,
      otherOwnerRaisesSeatsNow: 409,
      otherOwnerOpensThePayersPortal: 404,
      payerLeaves: 204,
      cancelsAskedOfPaddle: 1,
    });
  });
});

describe("who a Paddle customer is", () => {
  it("F4: each person's checkout is for their own Paddle customer, and the portal is theirs", async () => {
    await pay(OTHER, "pro", { last4: "4242" });
    const mine = await pay(OWNER, "pro", { last4: "1111" });
    const { rows } = await database.pool.query(
      "select count(distinct id)::int as people from billing_customers where provider_customer_id = $1",
      [ctmOf(OTHER)],
    );
    const portal = (await (await as(OWNER, "/v1/billing/portal")).json()) as {
      url?: string;
    };
    const seen = await overview(OWNER);
    expect({
      checkoutFor: mine.opened?.customer_id,
      peopleUnderOnePaddleCustomer: (rows[0] as { people: number }).people,
      portalSignedInAs: portal.url?.split("signed_in_as=")[1],
      card: seen.card?.last4,
    }).toEqual({
      checkoutFor: ctmOf(OWNER),
      peopleUnderOnePaddleCustomer: 1,
      portalSignedInAs: ctmOf(OWNER),
      card: "1111",
    });
  });
});

describe("a checkout changed after the API opened it", () => {
  it("F10a: a Pro checkout paid as Shed boxes is held as nothing", async () => {
    const refused = await openCheckout(NOT_STAFF, "shed");
    const { txn } = await openCheckout(NOT_STAFF, "pro");
    const sub = paddleSub(ctmOf(NOT_STAFF), "shed", 20);
    paddle.subscriptions.set(sub.id, sub);
    const answer = await deliver(
      "transaction.completed",
      paddleTxn(sub, { id: txn }),
      ago(5),
    );
    const held = (await overview(NOT_STAFF)).subscriptions
      .filter((each) => each.status === "active")
      .map((each) => `${each.product} x${each.quantity}`);
    expect({ refused: refused.status, answer, held }).toEqual({
      refused: 409,
      answer: 204,
      held: [],
    });
  });

  it("F10b: a checkout paid under another Paddle customer is ended and refunded, and moves nobody", async () => {
    await pay(OTHER, "pro");
    const { txn } = await openCheckout(OWNER, "pro");
    const sub = paddleSub("ctm_ofattacker", "pro", 1);
    paddle.subscriptions.set(sub.id, sub);
    expect(
      await deliver(
        "transaction.completed",
        paddleTxn(sub, { id: txn }),
        ago(5),
      ),
    ).toBe(204);

    const next = await openCheckout(OTHER, "shed");
    expect({
      endedAndRefunded: [
        askedOfPaddle("/cancel"),
        askedOfPaddle("/adjustments"),
      ],
      victimsNextCheckoutIsFor: next.sent?.customer_id,
    }).toEqual({
      endedAndRefunded: [1, 1],
      victimsNextCheckoutIsFor: ctmOf(OTHER),
    });
  });
});

describe("a change of interval at renewal", () => {
  it("R7: month to year and back leave Paddle's next bill on the paid month's end", async () => {
    const { sub } = await pay(OWNER, "pro");
    const paidEnd = sub.current_billing_period.ends_at;
    const [held] = (await overview(OWNER)).subscriptions;

    const toYear = await change(OWNER, held?.id, {
      interval: "year",
      when: "renewal",
    });
    expect(await deliver("subscription.updated", sub, ago(5))).toBe(204);
    const yearlyBillsOn = sub.next_billed_at;
    const [shown] = (await overview(OWNER)).subscriptions;

    const back = await change(OWNER, held?.id, { interval: "month" });
    expect(await deliver("subscription.updated", sub, ago(4))).toBe(204);
    const [after] = (await overview(OWNER)).subscriptions;
    expect({
      answers: [toYear.status, back.status],
      yearlyBillsOn,
      shown: shown?.currentPeriodEnd,
      monthlyBillsOn: sub.next_billed_at,
      after: after?.currentPeriodEnd,
    }).toEqual({
      answers: [200, 200],
      yearlyBillsOn: paidEnd,
      shown: paidEnd,
      monthlyBillsOn: paidEnd,
      after: paidEnd,
    });
  });
});

describe("a change of interval asked just before renewal", () => {
  it("S7: month to year 30 minutes before the paid end is refused, and Paddle still bills on it", async () => {
    const { sub } = await pay(OWNER, "pro", {
      ends: new Date(Date.now() + 30.05 * MINUTE_MS).toISOString(),
    });
    const paidEnd = sub.next_billed_at;
    const [held] = (await overview(OWNER)).subscriptions;
    paddle.gapAfterItems = 6_000;

    const answer = await change(OWNER, held?.id, {
      interval: "year",
      when: "renewal",
    });
    expect({
      answer: answer.status,
      words: ((await answer.json()) as { error?: { message?: string } }).error
        ?.message,
      billsOn: sub.next_billed_at,
      interval: sub.billing_cycle.interval,
    }).toEqual({
      answer: 409,
      words:
        "This subscription renews in the next few minutes. Change it after it renews.",
      billsOn: paidEnd,
      interval: "month",
    });
  });
});

describe("an address that passes from one account to another", () => {
  it("R3a: the second account gets a Paddle customer of its own, and a portal of its own", async () => {
    await pay(OWNER, "pro");
    clerk.addressOf.set(OWNER, "user_owner_moved");
    clerk.addressOf.set(NOT_STAFF, OWNER);
    const second = await openCheckout(NOT_STAFF, "pro");
    const sub = paddleSub(String(second.sent?.customer_id), "pro", 1);
    paddle.subscriptions.set(sub.id, sub);
    const answer = await deliver(
      "transaction.completed",
      paddleTxn(sub, { id: second.txn }),
      ago(5),
    );
    const portalOf = async (userId: string) =>
      (
        (await (await as(userId, "/v1/billing/portal")).json()) as {
          url?: string;
        }
      ).url?.split("signed_in_as=")[1];
    const { rows } = await database.pool.query(
      "select count(distinct provider_customer_id)::int as customers from billing_customers where user_id in ($1, $2)",
      [OWNER, NOT_STAFF],
    );
    expect({
      answer,
      paddleCustomers: (rows[0] as { customers: number }).customers,
      portals: [await portalOf(OWNER), await portalOf(NOT_STAFF)],
    }).toEqual({
      answer: 204,
      paddleCustomers: 2,
      portals: [ctmOf(OWNER), expect.not.stringMatching(ctmOf(OWNER))],
    });
  });

  it("R3a: while the account that holds the address has none verified, the checkout is refused", async () => {
    await pay(OWNER, "pro");
    clerk.addressOf.set(OWNER, null);
    clerk.addressOf.set(NOT_STAFF, OWNER);
    expect((await openCheckout(NOT_STAFF, "pro")).status).toBe(409);
  });

  it("R3b: after their address changes and they buy again, the payment page of the first plan still opens", async () => {
    await pay(OWNER, "pro");
    const before = await as(OWNER, "/v1/billing/portal");
    clerk.addressOf.set(OWNER, "user_owner_moved");
    const box = await pay(OWNER, "shed");
    const after = await as(OWNER, "/v1/billing/portal");
    expect({
      portal: [before.status, after.status],
      boxCheckoutFor: box.opened?.customer_id,
    }).toEqual({ portal: [200, 200], boxCheckoutFor: ctmOf(OWNER) });
  });
});

describe("4. a Paddle customer keeps an address its person gave up", () => {
  const addressOf = (customerId: string) =>
    [...paddle.customerOf].find(([, id]) => id === customerId)?.[0];

  it("N4: the next checkout leaves the customer on the old address", async () => {
    await pay(OWNER, "pro");
    clerk.addressOf.set(OWNER, "user_owner_moved");
    const box = await openCheckout(OWNER, "shed");

    expect({ status: box.status, address: addressOf(ctmOf(OWNER)) }).toEqual({
      status: 200,
      address: "verified address of user_owner_moved",
    });
  });

  it("N5: two addresses handed on in a row shut the third person out of checkout for good", async () => {
    await pay(OTHER, "pro");
    clerk.addressOf.set(OTHER, "user_b_moved");
    await pay(OWNER, "pro");
    clerk.addressOf.set(OWNER, OTHER);
    clerk.addressOf.set(NOT_STAFF, OWNER);

    const first = await openCheckout(NOT_STAFF, "pro");
    const others = [
      await openCheckout(OTHER, "shed"),
      await openCheckout(OWNER, "shed"),
    ].map((each) => each.status);
    const again = await openCheckout(NOT_STAFF, "pro");

    expect({ first: first.status, others, again: again.status }).toEqual({
      first: 200,
      others: [200, 200],
      again: 200,
    });
  });
});

describe("a sandbox purchase once the live key is in", () => {
  const invoices = async (userId: string) =>
    (await body(await as(userId, "/v1/billing/invoices"), "InvoiceList"))
      .invoices;
  const toLiveKey = async () => {
    paddle = new FakePaddle();
    paddle.sandbox = false;
    app = appWith(paddle);
    call = caller(app);
    await endSandboxPlans(database.db, log);
  };

  it("D22a: a refund taken in the sandbox leaves the first live Pro plan refundable", async () => {
    await pay(OWNER, "pro");
    const [sandbox] = await invoices(OWNER);
    const asked = await as(
      OWNER,
      `/v1/billing/invoices/${sandbox?.id}/refund`,
      { method: "POST" },
    );
    await toLiveKey();
    await pay(OWNER, "pro");
    const [plan] = (await overview(OWNER)).subscriptions;

    expect({
      asked: asked.status,
      refundableUntil: typeof plan?.refundableUntil,
    }).toEqual({ asked: 200, refundableUntil: "string" });
  });

  it("D22b: a sandbox payment is listed but never refundable, and asking queues nothing", async () => {
    await pay(OWNER, "pro");
    await toLiveKey();
    const [listed] = await invoices(OWNER);
    const asked = await as(OWNER, `/v1/billing/invoices/${listed?.id}/refund`, {
      method: "POST",
    });
    const { rows } = await database.pool.query(
      "select count(*)::int as calls from billing_calls",
    );

    expect({
      listed: listed?.refundableUntil,
      asked: asked.status,
      calls: (rows[0] as { calls: number }).calls,
    }).toEqual({ listed: undefined, asked: 409, calls: 0 });
  });
});

describe("an owner demoted while their first payment is applied", () => {
  it("finding 7: the payment waits for the demotion and does not make them the payer", async () => {
    const orgId = await createOrg();
    await database.pool.query(
      "insert into org_users (org_id, user_id, role) values ($1, $2, 'owner')",
      [orgId, OTHER],
    );
    const { txn } = await openCheckout(OTHER, "team", 3, orgId);
    const demotion = await database.pool.connect();
    try {
      await demotion.query("begin");
      await demotion.query(
        "update org_users set role = 'user' where org_id = $1 and user_id = $2",
        [orgId, OTHER],
      );
      const sub = paddleSub(ctmOf(OTHER), "team", 3);
      paddle.subscriptions.set(sub.id, sub);
      const applied = deliver(
        "transaction.completed",
        paddleTxn(sub, { id: txn }),
        ago(5),
      );
      for (let tries = 0; tries < 100; tries++) {
        const { rows } = await database.pool.query(
          "select count(*)::int as waiting from pg_stat_activity where wait_event_type = 'Lock'",
        );
        if ((rows[0] as { waiting: number }).waiting > 0) break;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      await demotion.query("commit");
      expect(await applied).toBe(204);
    } finally {
      demotion.release();
    }
    expect({
      payer: await paidBy(orgId),
      endedAndRefunded: [
        askedOfPaddle("/cancel"),
        askedOfPaddle("/adjustments"),
      ],
    }).toEqual({ payer: null, endedAndRefunded: [1, 1] });
  });
});

const FAR = "2099-01-01T00:00:00.000Z";

/** A checkout the API opened, paid after its custom data was set from the page's console. */
async function payWithCustomData(
  userId: string,
  product: BillingProductId,
  customData: Record<string, unknown>,
  orgId?: string,
) {
  const { status, txn: id } = await openCheckout(userId, product, 1, orgId);
  expect(status).toBe(200);
  const sub = paddleSub(ctmOf(userId), product, 1);
  sub.custom_data = customData;
  paddle.subscriptions.set(sub.id, sub);
  expect(
    await deliver("transaction.completed", paddleTxn(sub, { id }), ago(60)),
  ).toBe(204);
  return sub;
}

describe("custom data the buyer set on the checkout page", () => {
  it("R1a: one Team seat paid, 1000 named as held: the org's limit stays 1", async () => {
    const orgId = await createOrg();
    const sub = await payWithCustomData(
      OWNER,
      "team",
      {
        held: { product: "team", quantity: 1000, interval: "year", until: FAR },
      },
      orgId,
    );
    const paidFor = await seatLimit(database.db, orgId);

    const answer = await deliver("subscription.updated", sub, ago(30));

    const [held] = (await overview(OWNER, orgId)).subscriptions;
    expect({
      answer,
      paidFor,
      seats: await seatLimit(database.db, orgId),
      shown: [held?.quantity, held?.interval, held?.currentPeriodEnd],
    }).toEqual({
      answer: 204,
      paidFor: 1,
      seats: 1,
      shown: [1, "month", sub.current_billing_period.ends_at],
    });
  });

  it("R1b: Pro paid by someone not staff, Shed boxes named as held: Pro is what they hold", async () => {
    const sub = await payWithCustomData(NOT_STAFF, "pro", {
      held: {
        product: "shed_large",
        quantity: 20,
        interval: "month",
        until: FAR,
      },
    });
    const answer = await deliver("subscription.updated", sub, ago(30));
    const held = (await overview(NOT_STAFF)).subscriptions.map(
      (each) => `${each.product} x${each.quantity}`,
    );
    expect({ answer, held }).toEqual({ answer: 204, held: ["pro x1"] });
  });

  it("R1c: a bill date named on the checkout page leaves Paddle's next bill where it was", async () => {
    const sub = await payWithCustomData(NOT_STAFF, "pro", { bill_on: FAR });
    const paidEnd = sub.current_billing_period.ends_at;

    const created = await deliver("subscription.created", sub, ago(59));
    const billDatesAsked = paddle.calls
      .filter((each) => each.method === "PATCH" && each.body?.next_billed_at)
      .map((each) => each.body?.next_billed_at);
    const updated = await deliver("subscription.updated", sub, ago(30));

    const [held] = (await overview(NOT_STAFF)).subscriptions;
    expect({
      answers: [created, updated],
      billDatesAsked,
      paddleBillsNextOn: sub.next_billed_at,
      shownEnd: held?.currentPeriodEnd,
    }).toEqual({
      answers: [204, 204],
      billDatesAsked: [],
      paddleBillsNextOn: paidEnd,
      shownEnd: paidEnd,
    });
  });
});

describe("a change for renewal whose event has not arrived yet", () => {
  it("R2a: a change now straight after it is refused, so nothing is prorated from unpaid seats", async () => {
    const { id } = await team(5);
    const first = await change(OWNER, id, { quantity: 1000, when: "renewal" });
    const second = await change(OWNER, id, { quantity: 6 });
    expect({
      answers: [first.status, second.status],
      prorated: prorated(),
    }).toEqual({ answers: [200, 409], prorated: [] });
  });

  it("R2b: taking it back straight after gives Paddle the paid seats back", async () => {
    const { sub, id } = await team(5);
    await change(OWNER, id, { quantity: 1000, when: "renewal" });
    const asked = paddle.patches.length;
    const back = await change(OWNER, id, { quantity: 5 });
    expect({
      answer: back.status,
      askedOfPaddle: paddle.patches.length - asked,
      paddleBillsAtRenewal: sub.items[0]?.quantity,
    }).toEqual({ answer: 200, askedOfPaddle: 1, paddleBillsAtRenewal: 5 });
  });
});

describe("1. a change for renewal writes what is paid from our row, not Paddle's", () => {
  it("N1a: after a renewal that billed 1 seat, the org holds the 10 it had before", async () => {
    const { orgId, sub, id, paidEnd } = await teamHolding(10, 1);
    const renewal = renewed(sub, paidEnd);

    const asked = await change(OWNER, id, { quantity: 11, when: "renewal" });
    expect(asked.status).toBe(200);
    const written = heldAtPaddle(sub);
    expect(
      await deliver("subscription.updated", renewal, justAfter(paidEnd)),
    ).toBe(204);
    expect(await deliver("subscription.updated", sub, clockNow())).toBe(204);

    expect({
      writtenAsPaid: written,
      seats: await seatLimit(database.db, orgId),
    }).toEqual({ writtenAsPaid: 1, seats: 1 });
  });

  it("N1b: after a renewal that billed 10 seats, the org holds the 5 it had before", async () => {
    const { orgId, sub, id, paidEnd } = await teamHolding(5, 10);
    const renewal = renewed(sub, paidEnd);

    const asked = await change(OWNER, id, { quantity: 12, when: "renewal" });
    expect(asked.status).toBe(200);
    const written = heldAtPaddle(sub);
    expect(
      await deliver("subscription.updated", renewal, justAfter(paidEnd)),
    ).toBe(204);
    expect(await deliver("subscription.updated", sub, clockNow())).toBe(204);

    expect({
      writtenAsPaid: written,
      seats: await seatLimit(database.db, orgId),
    }).toEqual({ writtenAsPaid: 10, seats: 10 });
  });
});

describe("2. a failed renewal paid late drops the change held for it", () => {
  /** The renewal's payment fails, then is paid; the payment is delivered before the older snapshot. */
  async function curedOutOfOrder(
    sub: PaddleSub,
    paidEnd: Date,
    { snapshot = true } = {},
  ) {
    const renewal = renewed(sub, paidEnd);
    expect(
      await deliver(
        "subscription.past_due",
        { ...renewal, status: "past_due" },
        justAfter(paidEnd),
      ),
    ).toBe(204);
    const paidAt = clockNow();
    expect(
      await deliver(
        "transaction.completed",
        { ...paddleTxn(renewal), origin: "subscription_recurring" },
        paidAt,
      ),
    ).toBe(204);
    if (!snapshot) return;
    expect(
      await deliver(
        "subscription.updated",
        renewal,
        new Date(paidAt.getTime() - 1000),
      ),
    ).toBe(204);
  }

  it("N2a: 10 seats billed, 5 held, and the payer cannot put it right", async () => {
    const { orgId, sub, id, paidEnd } = await teamHolding(5, 10);
    await curedOutOfOrder(sub, paidEnd);

    const [plan] = (await overview(OWNER, orgId)).subscriptions;
    const now = await change(OWNER, id, { quantity: 10 });
    expect({
      status: plan?.status,
      billedByPaddle: sub.items[0]?.quantity,
      seats: await seatLimit(database.db, orgId),
      askingForThemNow: now.status,
    }).toEqual({
      status: "active",
      billedByPaddle: 10,
      seats: 10,
      askingForThemNow: 200,
    });
  });

  it("N2b: 1 seat billed, its snapshot on the way, and asking for that 1 seat gives 10", async () => {
    const { orgId, sub, id, paidEnd } = await teamHolding(10, 1);
    await curedOutOfOrder(sub, paidEnd, { snapshot: false });
    const before = await seatLimit(database.db, orgId);
    const calls = paddle.patches.length;

    const takenBack = await change(OWNER, id, { quantity: 1 });

    expect({
      before,
      takenBack: takenBack.status,
      askedOfPaddle: paddle.patches.length - calls,
      billedByPaddle: sub.items[0]?.quantity,
      seats: await seatLimit(database.db, orgId),
    }).toEqual({
      before: 1,
      takenBack: 200,
      askedOfPaddle: 0,
      billedByPaddle: 1,
      seats: 1,
    });
  });
});

describe("3. changes of one plan wait for each other holding a database connection each", () => {
  it("N3: twenty changes sent together leave no connection for anyone else", async () => {
    await pay(OWNER, "pro");
    const [plan] = (await overview(OWNER)).subscriptions;
    paddle.slowMs = 4_000;
    const changes = Array.from({ length: 20 }, () =>
      change(OWNER, plan?.id, { quantity: 1 }),
    );
    await new Promise((resolve) => setTimeout(resolve, 500));
    const { pool } = database;
    const inUse = pool.totalCount - pool.idleCount;
    const stranger = await as(OTHER, "/v1/billing");
    paddle.slowMs = 0;
    const statuses = new Set((await Promise.all(changes)).map((r) => r.status));

    expect([...statuses]).toEqual([200]);
    expect({ connectionsInUse: inUse, stranger: stranger.status }).toEqual({
      connectionsInUse: 0,
      stranger: 200,
    });
  }, 30_000);
});

describe("1. a change that is not a cut clears the cut while our row is a period behind", () => {
  it("P1: after a renewal that billed 1 seat, asking for 11 at the next one opens 10 seats before its event", async () => {
    const { orgId, sub, id, paidEnd } = await teamHolding(10, 1);
    renewed(sub, paidEnd);
    const before = await seatLimit(database.db, orgId);

    const asked = await change(OWNER, id, { quantity: 11, when: "renewal" });

    expect({
      before,
      asked: asked.status,
      paidAtPaddle: heldAtPaddle(sub),
      seats: await seatLimit(database.db, orgId),
      invitesMade: await invitesMade(orgId),
    }).toEqual({
      before: 1,
      asked: 200,
      paidAtPaddle: 1,
      seats: 1,
      invitesMade: 0,
    });
  });
});

describe("2. the cut the route records is never set right by what Paddle reports", () => {
  it("P2a: a raise Paddle took but whose answer was lost leaves an old cut in force, and asking again does not lift it", async () => {
    const { orgId, sub, id, paidEnd } = await teamHolding(10, 1);
    const renewal = renewed(sub, paidEnd);
    expect(
      await deliver("subscription.updated", renewal, justAfter(paidEnd)),
    ).toBe(204);

    loseNextAnswer();
    const raised = await change(OWNER, id, { quantity: 5 });
    expect(await deliver("subscription.updated", sub, clockNow())).toBe(204);
    const [plan] = (await overview(OWNER, orgId)).subscriptions;
    const afterEvent = await seatLimit(database.db, orgId);
    const again = await change(OWNER, id, { quantity: 5 });

    expect({
      raised: raised.status,
      billedByPaddle: sub.items[0]?.quantity,
      shownAsPaid: plan?.quantity,
      afterEvent,
      again: again.status,
      seats: await seatLimit(database.db, orgId),
    }).toEqual({
      raised: 500,
      billedByPaddle: 5,
      shownAsPaid: 5,
      afterEvent: 5,
      again: 200,
      seats: 5,
    });
  });

  it("P2b: a cut Paddle took but whose answer was lost leaves every paid seat open to invites until renewal", async () => {
    const { orgId, sub, id } = await team(10);

    loseNextAnswer();
    const cut = await change(OWNER, id, { quantity: 1, when: "renewal" });
    expect(await deliver("subscription.updated", sub)).toBe(204);
    const [plan] = (await overview(OWNER, orgId)).subscriptions;

    expect({
      cut: cut.status,
      atRenewal: plan?.atRenewal?.quantity,
      seats: await seatLimit(database.db, orgId),
      invitesMade: await invitesMade(orgId),
    }).toEqual({ cut: 500, atRenewal: 1, seats: 1, invitesMade: 0 });
  });
});

describe("a sale made with our own key that we have no record of", () => {
  it("D: leaves the subscription and the money alone, and refuses its event for Paddle to send again", async () => {
    const sub = paddleSub(ctmOf(OWNER), "pro", 1);
    paddle.subscriptions.set(sub.id, sub);
    const txn = paddleTxn(sub);

    const answered = await deliver("transaction.completed", txn);
    await new Promise((resolve) => setTimeout(resolve, 50));

    expect({
      answered,
      cancelled: askedOfPaddle("/cancel"),
      refunded: askedOfPaddle("/adjustments"),
    }).toEqual({ answered: 503, cancelled: 0, refunded: 0 });
  });
});
