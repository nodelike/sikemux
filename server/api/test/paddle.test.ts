import { createHmac } from "node:crypto";
import { readFileSync } from "node:fs";

import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { applyProviderEvents } from "../src/billing/apply.ts";
import type { PaddleApi } from "../src/billing/paddle-api.ts";
import { PaddleProvider } from "../src/billing/paddle.ts";
import type { Database } from "../src/db.ts";
import { emptyTables, migratedDatabase } from "./accounts.ts";
import { log } from "./support.ts";

interface Delivery {
  event_id: string;
  event_type: string;
  occurred_at: string;
  data: Record<string, unknown> & { subscription_id?: string; id: string };
}

// Every event Paddle's sandbox sent during a walk through the plans, in the order it sent them.
const WALK: Delivery[] = JSON.parse(
  readFileSync(new URL("./paddle-events.json", import.meta.url), "utf8"),
);

const SECRET = "pdl_ntfset_test_secret";
const INES = "00000000-0000-4000-8000-00000000a1e5";
const TOMAS = "00000000-0000-4000-8000-0000000070a5";
const PRIYA = "00000000-0000-4000-8000-00000000b71a";
const INES_PLAN = "sub_01m7h6pkekbvd0k65gd38d1ayq";
const TOMAS_PLAN = "sub_01m7h6sr15kpcxsgs30ffgh74q";

let database: Database;
let drop: () => Promise<void>;
let now: number;
let calls: { method: string; path: string; body: unknown }[];
let provider: PaddleProvider;

beforeAll(async () => {
  ({ database, drop } = await migratedDatabase());
});

afterAll(() => drop());

beforeEach(async () => {
  await emptyTables(database);
  await database.pool.query("truncate orgs, billing_events cascade");
  calls = [];
  const api = {
    sandbox: true,
    request: async (method: string, path: string, body: unknown) => {
      calls.push({ method, path, body });
      return {};
    },
    list: async () => [],
  } as unknown as PaddleApi;
  provider = new PaddleProvider({
    api,
    apiOrigin: "https://api.sikemux.com",
    log,
    webhookSecret: SECRET,
    now: () => now,
  });
  await customers();
});

async function customers() {
  await database.db
    .insertInto("users")
    .values([{ id: "user_ines" }, { id: "user_tomas" }, { id: "user_priya" }])
    .execute();
  const org = await database.db
    .insertInto("orgs")
    .values({ name: "Acme", created_by: "user_tomas" })
    .returning("id")
    .executeTakeFirstOrThrow();
  await database.db
    .insertInto("org_users")
    .values({
      org_id: org.id,
      user_id: "user_tomas",
      role: "owner",
      invited_by: null,
    })
    .execute();
  await database.db
    .insertInto("billing_customers")
    .values([
      { id: INES, user_id: "user_ines", provider: "paddle" },
      { id: TOMAS, org_id: org.id, provider: "paddle" },
      { id: PRIYA, user_id: "user_priya", provider: "paddle" },
    ])
    .execute();
}

function signed(delivery: Delivery, secret = SECRET) {
  const body = JSON.stringify(delivery);
  const ts = Math.floor(Date.parse(delivery.occurred_at) / 1000);
  const h1 = createHmac("sha256", secret).update(`${ts}:${body}`).digest("hex");
  const headers: Record<string, string> = {
    "paddle-signature": `ts=${ts};h1=${h1}`,
  };
  return { body, header: (name: string) => headers[name] };
}

async function deliver(deliveries: Delivery[]) {
  for (const delivery of deliveries) {
    now = Date.parse(delivery.occurred_at) + 1000;
    const events = await provider.readEvents(signed(delivery));
    if (!events) throw new Error(`${delivery.event_id} was not believed`);
    await applyProviderEvents(database.db, provider, events, log);
  }
}

function only<T>(found: T | undefined): T {
  if (found === undefined) throw new Error("The walk has no such event.");
  return found;
}

const of = (plan: string) =>
  WALK.filter(
    (delivery) => (delivery.data.subscription_id ?? delivery.data.id) === plan,
  );

async function plans(customerId: string) {
  return database.db
    .selectFrom("subscriptions")
    .select([
      "product",
      "interval",
      "quantity",
      "status",
      "cancel_at_period_end",
      "next_quantity",
    ])
    .where("customer_id", "=", customerId)
    .execute();
}

async function invoices(customerId: string) {
  return database.db
    .selectFrom("invoices")
    .select(["reason", "amount", "currency", "status", "lines"])
    .where("customer_id", "=", customerId)
    .orderBy("at")
    .execute();
}

describe("Paddle's sandbox events", () => {
  it("keep a Pro plan bought monthly, moved to yearly, cancelled, resumed, then refunded to end at the period's close", async () => {
    await deliver(of(INES_PLAN));
    expect(await plans(INES)).toEqual([
      {
        product: "pro",
        interval: "year",
        quantity: 1,
        status: "active",
        cancel_at_period_end: true,
        next_quantity: null,
      },
    ]);
    expect(await invoices(INES)).toEqual([
      {
        reason: "first_payment",
        amount: "800",
        currency: "usd",
        status: "refunded",
        lines: [{ product: "pro", quantity: 1 }],
      },
      {
        reason: "change",
        amount: "7200",
        currency: "usd",
        status: "paid",
        lines: [{ product: "pro", quantity: 1 }],
      },
    ]);
  });

  it("keep a Team plan's seats through a rise, a cut now and a cut held to renewal", async () => {
    await deliver(of(TOMAS_PLAN));
    expect(await plans(TOMAS)).toEqual([
      {
        product: "team",
        interval: "month",
        quantity: 2,
        status: "active",
        cancel_at_period_end: false,
        next_quantity: 1,
      },
    ]);
    expect(
      (await invoices(TOMAS)).map(({ reason, amount }) => ({ reason, amount })),
    ).toEqual([
      { reason: "first_payment", amount: "8000" },
      { reason: "change", amount: "4000" },
    ]);
  });

  it("start a Shed box only once a declined card is replaced and paid", async () => {
    await deliver(
      WALK.filter(
        (delivery) =>
          JSON.stringify(delivery.data).includes(PRIYA) ||
          delivery.event_type === "transaction.payment_failed",
      ),
    );
    expect(await plans(PRIYA)).toEqual([
      expect.objectContaining({ product: "shed", status: "active" }),
    ]);
    expect(await invoices(PRIYA)).toEqual([
      expect.objectContaining({
        reason: "first_payment",
        amount: "1900",
        status: "paid",
      }),
    ]);
  });

  it("apply each event once when Paddle sends it again", async () => {
    await deliver([...of(INES_PLAN), ...of(INES_PLAN)]);
    expect(await invoices(INES)).toHaveLength(2);
  });

  it("refund a checkout opened outside ours rather than start a plan", async () => {
    const first = only(
      of(INES_PLAN).find(
        (delivery) => delivery.event_type === "transaction.completed",
      ),
    );
    await deliver([{ ...first, data: { ...first.data, origin: "web" } }]);
    expect(await plans(INES)).toEqual([]);
    expect(calls).toEqual([
      expect.objectContaining({
        method: "POST",
        path: `/subscriptions/${INES_PLAN}/cancel`,
      }),
      {
        method: "POST",
        path: "/adjustments",
        body: expect.objectContaining({
          action: "refund",
          transaction_id: first.data.id,
        }),
      },
    ]);
  });

  it("clear a plan's refund request when Paddle rejects the refund", async () => {
    const walk = of(INES_PLAN);
    const approved = only(walk.at(-1));
    await deliver(walk.slice(0, -1));
    await database.db
      .updateTable("invoices")
      .set({ refund_requested_at: new Date() })
      .where("customer_id", "=", INES)
      .execute();
    await deliver([
      { ...approved, data: { ...approved.data, status: "rejected" } },
    ]);
    expect((await invoices(INES)).map(({ status }) => status)).toEqual([
      "paid",
      "paid",
    ]);
    const requested = await database.db
      .selectFrom("invoices")
      .select("refund_requested_at")
      .where("customer_id", "=", INES)
      .execute();
    expect(requested.every((row) => row.refund_requested_at === null)).toBe(
      true,
    );
  });

  it("ignore a delivery signed with another secret", async () => {
    const first = only(of(INES_PLAN)[0]);
    now = Date.parse(first.occurred_at);
    expect(
      await provider.readEvents(signed(first, "pdl_ntfset_other")),
    ).toBeNull();
  });
});
