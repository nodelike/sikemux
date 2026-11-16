import type { Services } from "../app.ts";
import type {
  BillingOverview,
  Invoice,
  InvoiceList,
  Subscription,
} from "@sikemux/protocol";
import { Hono, type Context } from "hono";
import type { Kysely, Selectable } from "kysely";

import { audit } from "../audit.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import {
  ApiFailure,
  badRequest,
  forbidden,
  notFound,
  readBody,
  unauthorized,
  type Env,
} from "../http.ts";
import { perUser } from "../limits.ts";
import type { Logger } from "../log.ts";
import { isStaff } from "../account/staff.ts";
import { orgRole } from "../orgs/roles.ts";
import { requireBillingAccess } from "./access.ts";
import { allowance } from "./allowance.ts";
import { applyProviderEvents } from "./apply.ts";
import { CATALOG, REFUND_DAYS, product, unitPrice } from "./catalog.ts";
import type {
  BillingProvider,
  HeldSubscription,
  ProviderRequest,
} from "./provider.ts";
import { StandInProvider } from "./stand-in.ts";

const DAY_MS = 24 * 60 * 60_000;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

type Payer = { userId: string } | { orgId: string };
type SubscriptionRow = Selectable<Tables["subscriptions"]>;

function needProvider(provider: BillingProvider | null): BillingProvider {
  if (!provider)
    throw new ApiFailure(
      503,
      "unavailable",
      "No payment provider is set up, so nothing can be bought or changed.",
    );
  return provider;
}

/** Reads a provider's webhook request and applies its events: the one way billing records change. */
async function receive(
  db: Kysely<Tables>,
  provider: BillingProvider,
  request: ProviderRequest,
  log: Logger,
) {
  const events = await provider.readEvents(request);
  if (!events) throw unauthorized("The provider's signature is not valid.");
  await applyProviderEvents(db, provider.name, events, log);
}

/** The payment provider's webhook. Like /v1/hooks it sits outside the per-address limit. */
export function billingProviderRoutes({
  database: { db },
  billing: { provider },
}: Pick<Services, "database" | "billing">) {
  return new Hono<Env>().post("/", async (c) => {
    const ready = needProvider(provider);
    await receive(
      db,
      ready,
      { body: await c.req.text(), header: (name) => c.req.header(name) },
      c.get("log"),
    );
    return c.body(null, 204);
  });
}

function subscriptionBody(row: SubscriptionRow): Subscription {
  return {
    id: row.id,
    product: row.product,
    interval: row.interval,
    quantity: row.quantity,
    status: row.status,
    currentPeriodEnd: row.current_period_end.toISOString(),
    cancelAtPeriodEnd: row.cancel_at_period_end,
    startedAt: row.started_at.toISOString(),
    ...(row.ended_at ? { endedAt: row.ended_at.toISOString() } : {}),
  };
}

function held(
  row: SubscriptionRow,
  providerCustomerId: string | null,
): HeldSubscription {
  return {
    providerSubscriptionId: row.provider_subscription_id,
    providerCustomerId,
    product: row.product,
    interval: row.interval,
    quantity: row.quantity,
    currentPeriodEnd: row.current_period_end,
    cancelAtPeriodEnd: row.cancel_at_period_end,
  };
}

function customersOf(db: Kysely<Tables>, payer: Payer) {
  return "userId" in payer
    ? db.selectFrom("billing_customers").where("user_id", "=", payer.userId)
    : db.selectFrom("billing_customers").where("org_id", "=", payer.orgId);
}

/** Only a subscription's first payment is refunded, in full and within the refund window. */
function refundableUntil(
  row: Selectable<Tables["invoices"]>,
  now = Date.now(),
): Date | null {
  if (row.status !== "paid" || row.reason !== "first_payment") return null;
  const until = row.at.getTime() + REFUND_DAYS * DAY_MS;
  return until > now ? new Date(until) : null;
}

/**
 * What a person or an org buys and pays for. A person acts for their own billing; only an org's
 * owner acts for the org's. Until billing is live, only staff get in.
 */
export function billingRoutes({
  database: { db },
  verifier,
  limiter,
  appOrigin,
  log,
  billing: { provider, live },
}: Pick<
  Services,
  "database" | "verifier" | "limiter" | "appOrigin" | "log" | "billing"
>) {
  const app = new Hono<AuthEnv>();

  if (provider instanceof StandInProvider) {
    provider.deliverTo((request) => receive(db, provider, request, log));
    app.get("/stand-in/checkout/:token", async (c) => {
      const back = await provider.complete(c.req.param("token"));
      if (!back) throw notFound("No checkout is waiting at this address.");
      return c.redirect(back, 303);
    });
  }

  app.use(requireIdentity(verifier, db), requireBillingAccess(db, live));

  /** The payer a request is about: the person, or with `orgId`, an org they own. */
  const payerOf = async (
    c: Context<AuthEnv>,
    orgId: string | undefined,
  ): Promise<Payer> => {
    const { userId } = c.get("identity");
    if (orgId === undefined) return { userId };
    if (!UUID.test(orgId)) throw badRequest("org is not an org id.");
    if ((await orgRole(db, orgId, userId)) !== "owner")
      throw forbidden("Only the org's owner acts for its billing.");
    return { orgId };
  };

  /** A subscription the signed-in person may act for, or a 404 that says nothing of others'. */
  const subscriptionFor = async (c: Context<AuthEnv>) => {
    const id = c.req.param("subscriptionId") ?? "";
    const row = UUID.test(id)
      ? await db
          .selectFrom("subscriptions")
          .innerJoin(
            "billing_customers",
            "billing_customers.id",
            "subscriptions.customer_id",
          )
          .selectAll("subscriptions")
          .select([
            "billing_customers.user_id",
            "billing_customers.org_id",
            "billing_customers.provider_customer_id",
          ])
          .where("subscriptions.id", "=", id)
          .executeTakeFirst()
      : undefined;
    const { userId } = c.get("identity");
    const mine =
      row &&
      (row.user_id === userId ||
        (row.org_id !== null &&
          (await orgRole(db, row.org_id, userId)) === "owner"));
    if (!row || !mine) throw notFound("No such subscription of yours.");
    if (row.status === "ended")
      throw new ApiFailure(409, "conflict", "The subscription has ended.");
    return row;
  };

  const reread = async (id: string) =>
    subscriptionBody(
      await db
        .selectFrom("subscriptions")
        .selectAll()
        .where("id", "=", id)
        .executeTakeFirstOrThrow(),
    );

  /** The payer's card as the provider shows it; a provider that cannot answer leaves it out. */
  const cardOf = async (payer: Payer) => {
    if (!provider) return null;
    const customer = await customersOf(db, payer)
      .select("provider_customer_id")
      .where("provider", "=", provider.name)
      .executeTakeFirst();
    if (!customer?.provider_customer_id) return null;
    try {
      return await provider.card(customer.provider_customer_id);
    } catch (err) {
      log.warn({ err, provider: provider.name }, "could not ask for the card");
      return null;
    }
  };

  /** An invoice the signed-in person may act for, or a 404 that says nothing of others'. */
  const invoiceFor = async (c: Context<AuthEnv>) => {
    const id = c.req.param("invoiceId") ?? "";
    const row = UUID.test(id)
      ? await db
          .selectFrom("invoices")
          .innerJoin(
            "billing_customers",
            "billing_customers.id",
            "invoices.customer_id",
          )
          .selectAll("invoices")
          .select(["billing_customers.user_id", "billing_customers.org_id"])
          .where("invoices.id", "=", id)
          .executeTakeFirst()
      : undefined;
    const { userId } = c.get("identity");
    const mine =
      row &&
      (row.user_id === userId ||
        (row.org_id !== null &&
          (await orgRole(db, row.org_id, userId)) === "owner"));
    if (!row || !mine) throw notFound("No such invoice of yours.");
    return row;
  };

  app.get("/catalog", (c) => c.json(CATALOG));

  app.get("/", async (c) => {
    const payer = await payerOf(c, c.req.query("org"));
    const customers = await customersOf(db, payer).select("id").execute();
    const rows = customers.length
      ? await db
          .selectFrom("subscriptions")
          .selectAll()
          .where(
            "customer_id",
            "in",
            customers.map((customer) => customer.id),
          )
          .orderBy((eb) => eb("status", "=", "ended"))
          .orderBy("started_at", "desc")
          .execute()
      : [];
    const card = await cardOf(payer);
    const body: BillingOverview = {
      allowance: await allowance(db, payer, { live }),
      subscriptions: rows.map(subscriptionBody),
      live,
      canBuy: provider !== null,
      ...(card ? { card } : {}),
    };
    return c.json(body);
  });

  app.post("/checkout", perUser(limiter, "billing-checkout", 20), async (c) => {
    const ready = needProvider(provider);
    const request = await readBody(c, "CheckoutRequest");
    const payer = await payerOf(c, request.orgId);
    const { userId } = c.get("identity");
    const sold = product(request.product);
    if (sold.payer === "person" && "orgId" in payer)
      throw badRequest(`${sold.name} is bought by a person, not an org.`);
    if (sold.payer === "org" && "userId" in payer)
      throw badRequest(`${sold.name} is bought for an org: send its orgId.`);
    const unit = unitPrice(sold.id, request.interval);
    if (!unit)
      throw badRequest(`${sold.name} is not sold by the ${request.interval}.`);
    if (request.quantity > sold.maxQuantity)
      throw badRequest(
        `${sold.name} is sold up to ${sold.maxQuantity} at once.`,
      );
    if (sold.kind === "shed" && !(await isStaff(db, userId)))
      throw new ApiFailure(
        409,
        "conflict",
        "The Shed is not on sale yet: no box can be provisioned.",
      );

    const customer = await db.transaction().execute(async (trx) => {
      if ("userId" in payer)
        await trx
          .insertInto("users")
          .values({ id: payer.userId })
          .onConflict((oc) => oc.column("id").doNothing())
          .execute();
      const values =
        "userId" in payer
          ? { user_id: payer.userId, provider: ready.name }
          : { org_id: payer.orgId, provider: ready.name };
      await trx
        .insertInto("billing_customers")
        .values(values)
        .onConflict((oc) => oc.doNothing())
        .execute();
      const found = await customersOf(trx, payer)
        .select(["id", "provider_customer_id"])
        .where("provider", "=", ready.name)
        .forUpdate()
        .executeTakeFirstOrThrow();
      if (sold.kind === "plan") {
        const holding = await trx
          .selectFrom("subscriptions")
          .select("id")
          .where("customer_id", "=", found.id)
          .where("product", "=", sold.id)
          .where("status", "<>", "ended")
          .executeTakeFirst();
        if (holding)
          throw new ApiFailure(
            409,
            "conflict",
            `This already holds ${sold.name}; change that subscription instead.`,
          );
      }
      return found;
    });

    const checkout = await ready.checkout({
      customerRef: customer.id,
      providerCustomerId: customer.provider_customer_id,
      product: sold.id,
      interval: request.interval,
      quantity: request.quantity,
      unit,
      returnUrl: `${appOrigin}/billing`,
    });
    await audit(db, {
      userId,
      actor: `user:${userId}`,
      action: "billing.checkout_opened",
      subject: customer.id,
      detail: {
        product: sold.id,
        interval: request.interval,
        quantity: request.quantity,
      },
    });
    return c.json(checkout);
  });

  app.patch(
    "/subscriptions/:subscriptionId",
    perUser(limiter, "billing-change", 20),
    async (c) => {
      const ready = needProvider(provider);
      const row = await subscriptionFor(c);
      const change = await readBody(c, "SubscriptionChange");
      const next = {
        product: change.product ?? row.product,
        interval: change.interval ?? row.interval,
        quantity: change.quantity ?? row.quantity,
      };
      const from = product(row.product);
      const to = product(next.product);
      if (from.kind !== "shed" || to.kind !== "shed") {
        if (to.id !== from.id)
          throw badRequest("A plan changes only its quantity or interval.");
      }
      const unit = unitPrice(to.id, next.interval);
      if (!unit)
        throw badRequest(`${to.name} is not sold by the ${next.interval}.`);
      if (next.quantity > to.maxQuantity)
        throw badRequest(`${to.name} is sold up to ${to.maxQuantity} at once.`);
      if (to.id === "team" && row.org_id) {
        const seated = await db
          .selectFrom("org_users")
          .select((eb) => eb.fn.countAll<string>().as("count"))
          .where("org_id", "=", row.org_id)
          .executeTakeFirstOrThrow();
        if (next.quantity < Number(seated.count))
          throw badRequest(
            `${seated.count} people hold seats; remove some before paying for fewer.`,
          );
      }
      const same =
        next.product === row.product &&
        next.interval === row.interval &&
        next.quantity === row.quantity;
      if (!same) {
        await ready.change(held(row, row.provider_customer_id), {
          ...next,
          unit,
        });
        const { userId } = c.get("identity");
        await audit(db, {
          userId,
          actor: `user:${userId}`,
          action: "billing.subscription_changed",
          subject: row.id,
          detail: next,
        });
      }
      return c.json(await reread(row.id));
    },
  );

  for (const [path, cancel] of [
    ["cancel", true],
    ["resume", false],
  ] as const)
    app.post(
      `/subscriptions/:subscriptionId/${path}`,
      perUser(limiter, "billing-change", 20),
      async (c) => {
        const ready = needProvider(provider);
        const row = await subscriptionFor(c);
        if (row.cancel_at_period_end !== cancel) {
          const subscription = held(row, row.provider_customer_id);
          await (cancel
            ? ready.cancel(subscription, "period_end")
            : ready.resume(subscription));
          const { userId } = c.get("identity");
          await audit(db, {
            userId,
            actor: `user:${userId}`,
            action: cancel
              ? "billing.subscription_canceled"
              : "billing.subscription_resumed",
            subject: row.id,
          });
        }
        return c.json(await reread(row.id));
      },
    );

  app.get("/invoices", async (c) => {
    const payer = await payerOf(c, c.req.query("org"));
    const ids = (await customersOf(db, payer).select("id").execute()).map(
      (customer) => customer.id,
    );
    if (!ids.length) return c.json({ invoices: [] } satisfies InvoiceList);
    const rows = await db
      .selectFrom("invoices")
      .selectAll()
      .where("customer_id", "in", ids)
      .orderBy("at", "desc")
      .limit(200)
      .execute();
    const body: InvoiceList = { invoices: rows.map(invoiceBody) };
    return c.json(body);
  });

  app.post(
    "/invoices/:invoiceId/refund",
    perUser(limiter, "billing-refund", 5),
    async (c) => {
      const ready = needProvider(provider);
      const { userId } = c.get("identity");
      const row = await invoiceFor(c);
      const subscriptionId = row.subscription_id;
      if (!subscriptionId || !refundableUntil(row))
        throw new ApiFailure(
          409,
          "conflict",
          `Only a subscription's first payment is refunded, within ${REFUND_DAYS} days of it.`,
        );
      const subscription = await db
        .selectFrom("subscriptions")
        .innerJoin(
          "billing_customers",
          "billing_customers.id",
          "subscriptions.customer_id",
        )
        .selectAll("subscriptions")
        .select("billing_customers.provider_customer_id")
        .where("subscriptions.id", "=", subscriptionId)
        .executeTakeFirstOrThrow();
      if (subscription.status !== "ended")
        await ready.cancel(
          held(subscription, subscription.provider_customer_id),
          "now",
        );
      await ready.refund(row.provider_payment_id);
      await audit(db, {
        userId,
        actor: `user:${userId}`,
        action: "billing.refunded",
        subject: row.id,
      });
      const after = await db
        .selectFrom("invoices")
        .selectAll()
        .where("id", "=", row.id)
        .executeTakeFirstOrThrow();
      return c.json(invoiceBody(after));
    },
  );

  app.get("/invoices/:invoiceId/receipt", async (c) => {
    const ready = needProvider(provider);
    const row = await invoiceFor(c);
    const receipt = await ready.receipt(row.provider_payment_id);
    if (!receipt)
      throw notFound("The provider has no receipt for this payment.");
    return c.json(receipt);
  });

  app.get("/portal", async (c) => {
    const ready = needProvider(provider);
    const payer = await payerOf(c, c.req.query("org"));
    const customer = await customersOf(db, payer)
      .select("provider_customer_id")
      .where("provider", "=", ready.name)
      .executeTakeFirst();
    const portal = customer?.provider_customer_id
      ? await ready.portal(customer.provider_customer_id)
      : null;
    if (!portal)
      throw notFound("The provider has no page for this payer to manage.");
    return c.json(portal);
  });

  return app;
}

function invoiceBody(row: Selectable<Tables["invoices"]>): Invoice {
  const until = refundableUntil(row);
  return {
    id: row.id,
    ...(row.subscription_id ? { subscriptionId: row.subscription_id } : {}),
    product: row.product,
    quantity: row.quantity,
    reason: row.reason,
    total: { amount: Number(row.amount), currency: row.currency },
    status: row.status,
    at: row.at.toISOString(),
    ...(until ? { refundableUntil: until.toISOString() } : {}),
  };
}
