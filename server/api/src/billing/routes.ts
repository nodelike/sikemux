import type { Services } from "../app.ts";
import type {
  BillingOverview,
  ChangePreview,
  PlanPayer,
  Invoice,
  InvoiceList,
  PaddleCheckout,
  Subscription,
} from "@sikemux/protocol";
import { Hono, type Context } from "hono";
import { type Kysely, type Selectable } from "kysely";

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
import { orgRole } from "../orgs/roles.ts";
import { requireBillingAccess } from "./access.ts";
import { allowance } from "./allowance.ts";
import { applyProviderEvents } from "./apply.ts";
import { openerRole, refusal } from "./checks.ts";
import { ownCustomer } from "./customers.ts";
import { makeDueCalls } from "./calls.ts";
import { CATALOG, product, unitPrice } from "./catalog.ts";
import {
  heldSubscription,
  type BillingProvider,
  type NotNow,
  type ProviderRequest,
} from "./provider.ts";
import { PaddleProvider } from "./paddle.ts";
import {
  askRefund,
  planRefund,
  refundPending,
  type PlanRefund,
} from "./refunds.ts";
import { lockSeats, seatsHeld } from "./seats.ts";
import { StandInProvider } from "./stand-in.ts";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

type Payer = { userId: string } | { orgId: string };
type SubscriptionRow = Selectable<Tables["subscriptions"]>;

const changing = new Map<string, Promise<unknown>>();

/**
 * Runs the changes of one subscription one after another. They wait here rather than on a
 * database lock, so a change waiting for its turn, or at the provider, holds no connection.
 */
async function inTurn<T>(key: string, work: () => Promise<T>): Promise<T> {
  const turn = (changing.get(key) ?? Promise.resolve()).then(work);
  const settled = turn.catch(() => undefined);
  changing.set(key, settled);
  try {
    return await turn;
  } finally {
    if (changing.get(key) === settled) changing.delete(key);
  }
}

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
  await applyProviderEvents(db, provider, events, log);
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

function subscriptionBody(
  row: SubscriptionRow,
  refund: PlanRefund,
): Subscription {
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
    ...(row.next_product && row.next_interval && row.next_quantity
      ? {
          atRenewal: {
            product: row.next_product,
            interval: row.next_interval,
            quantity: row.next_quantity,
          },
        }
      : {}),
    ...(refund.until ? { refundableUntil: refund.until.toISOString() } : {}),
    ...(refund.requestedAt
      ? { refundRequestedAt: refund.requestedAt.toISOString() }
      : {}),
    ...(refund.amount && (refund.until || refund.requestedAt)
      ? { refundAmount: refund.amount }
      : {}),
  };
}

/** A subscription as the API shows it, with where its plan's refund stands. */
async function subscriptionWithRefund(
  db: Kysely<Tables>,
  provider: string | undefined,
  row: SubscriptionRow,
): Promise<Subscription> {
  const refund =
    row.status === "ended"
      ? { covers: [] }
      : await planRefund(
          db,
          provider,
          row.customer_id,
          row.provider_subscription_id,
        );
  return subscriptionBody(row, refund);
}

function customersOf(db: Kysely<Tables>, payer: Payer) {
  return "userId" in payer
    ? db.selectFrom("billing_customers").where("user_id", "=", payer.userId)
    : db.selectFrom("billing_customers").where("org_id", "=", payer.orgId);
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
  clerk,
  billing: { provider, live },
}: Pick<
  Services,
  | "database"
  | "verifier"
  | "limiter"
  | "appOrigin"
  | "log"
  | "clerk"
  | "billing"
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

  app.get("/paddle", async (c) => {
    if (!(provider instanceof PaddleProvider))
      throw notFound("Billing does not go through Paddle.");
    const page = await provider.checkoutPage();
    return c.json({
      clientToken: page.clientToken,
      environment: page.sandbox ? "sandbox" : "production",
    } satisfies PaddleCheckout);
  });

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
            "billing_customers.paid_by",
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

  /** Refuses acting on an org's plan with the payment method of someone who no longer owns it. */
  const requirePayer = (
    c: Context<AuthEnv>,
    row: { org_id: string | null; paid_by: string | null },
  ) => {
    if (row.org_id !== null && row.paid_by !== c.get("identity").userId)
      throw new ApiFailure(
        409,
        "conflict",
        "This plan was paid for by someone who no longer owns the org, so it ends at the period's end. Buy Team for the org once it has ended.",
      );
  };

  const reread = async (id: string) =>
    subscriptionWithRefund(
      db,
      provider?.name,
      await db
        .selectFrom("subscriptions")
        .selectAll()
        .where("id", "=", id)
        .executeTakeFirstOrThrow(),
    );

  const refusePending = async (row: SubscriptionRow) => {
    if (await refundPending(db, row.customer_id, row.provider_subscription_id))
      throw new ApiFailure(
        409,
        "conflict",
        "A refund of this plan is waiting; it ends when the refund is paid.",
      );
  };

  /** Asks for a plan's refund, and makes the provider calls it leads to before answering. */
  const refundPlan = async (
    c: Context<AuthEnv>,
    ready: BillingProvider,
    row: { customer_id: string; provider_subscription_id: string },
    subject: string,
  ) => {
    const outcome = await askRefund(
      db,
      ready.name,
      row.customer_id,
      row.provider_subscription_id,
    );
    if (outcome === "already") return;
    const { userId } = c.get("identity");
    await audit(db, {
      userId,
      actor: `user:${userId}`,
      action: "billing.refund_requested",
      subject,
    });
    await makeDueCalls(db, ready, c.get("log"));
  };

  /** The payer's plan that has not ended, Pro or Team, as the provider acts on it. */
  const openPlanOf = async (
    payer: Payer,
    userId: string,
    ready: BillingProvider,
  ) => {
    let query = customersOf(db, payer);
    if ("orgId" in payer)
      query = query.where("billing_customers.paid_by", "=", userId);
    const row = await query
      .innerJoin(
        "subscriptions",
        "subscriptions.customer_id",
        "billing_customers.id",
      )
      .selectAll("subscriptions")
      .select("billing_customers.provider_customer_id")
      .where("billing_customers.provider", "=", ready.name)
      .where("subscriptions.status", "!=", "ended")
      .where("subscriptions.product", "in", ["pro", "team"])
      .orderBy("subscriptions.started_at", "desc")
      .executeTakeFirst();
    return row ? heldSubscription(row, row.provider_customer_id) : null;
  };

  /** The card the payer's plan pays with, as the provider shows it; a provider that cannot answer leaves it out. */
  const cardOf = async (payer: Payer, userId: string) => {
    if (!provider) return null;
    const plan = await openPlanOf(payer, userId, provider);
    if (!plan) return null;
    try {
      return await provider.card(plan);
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
          .select([
            "billing_customers.user_id",
            "billing_customers.org_id",
            "billing_customers.provider",
          ])
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

  const invoiceWithRefund = async (row: Selectable<Tables["invoices"]>) => {
    const refundable =
      row.reason === "first_payment" &&
      row.status === "paid" &&
      !row.refund_requested_at;
    const refund = refundable
      ? await planRefund(
          db,
          provider?.name,
          row.customer_id,
          row.provider_subscription_id,
        )
      : undefined;
    return invoiceBody(row, refund?.until);
  };

  app.get("/catalog", (c) => c.json(CATALOG));

  app.get("/", async (c) => {
    const payer = await payerOf(c, c.req.query("org"));
    const { userId } = c.get("identity");
    const customers = await customersOf(db, payer)
      .select(["id", "provider", "paid_by"])
      .execute();
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
    const card = await cardOf(payer, userId);
    const planPayer = (): PlanPayer | null => {
      if ("userId" in payer || rows.every((row) => row.status === "ended"))
        return null;
      const customer =
        customers.find((each) => each.provider === provider?.name) ??
        customers[0];
      if (customer?.paid_by === userId) return "you";
      return customer?.paid_by ? "another_owner" : "departed";
    };
    const paidBy = planPayer();
    const body: BillingOverview = {
      allowance: await allowance(db, payer, { live }),
      subscriptions: await Promise.all(
        rows.map((row) => subscriptionWithRefund(db, provider?.name, row)),
      ),
      live,
      canBuy: provider !== null && (paidBy === null || paidBy === "you"),
      ...(paidBy ? { payer: paidBy } : {}),
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

    const customer = await db.transaction().execute(async (trx) => {
      await trx
        .insertInto("users")
        .values({ id: userId })
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
      const orgId = "orgId" in payer ? payer.orgId : null;
      const role = await openerRole(trx, orgId, userId);
      const found = await customersOf(trx, payer)
        .select(["id", "paid_by"])
        .where("provider", "=", ready.name)
        .forUpdate()
        .executeTakeFirstOrThrow();
      const refused = await refusal(trx, {
        customerId: found.id,
        orgId,
        paidBy: found.paid_by,
        openedBy: userId,
        openerRole: role,
        product: sold.id,
        quantity: request.quantity,
      });
      if (refused) throw new ApiFailure(409, "conflict", refused);
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

    const providerCustomerId = await ownCustomer(
      db,
      ready,
      clerk,
      c.get("log"),
      userId,
    );
    const checkout = await ready.checkout({
      providerCustomerId,
      product: sold.id,
      interval: request.interval,
      quantity: request.quantity,
      unit,
      returnUrl: `${appOrigin}/billing`,
    });
    await db
      .insertInto("billing_checkouts")
      .values({
        provider: ready.name,
        provider_checkout_id: checkout.providerCheckoutId,
        customer_id: customer.id,
        opened_by: userId,
        provider_customer_id: providerCustomerId,
        product: sold.id,
        interval: request.interval,
        quantity: request.quantity,
      })
      .execute();
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
    return c.json({ url: checkout.url });
  });

  const heldForRenewal = () =>
    new ApiFailure(
      409,
      "conflict",
      "A change is waiting for renewal. Take it back before changing anything now.",
    );

  /** The change a request asks of its subscription, refused here for whatever the change would refuse. */
  const askedChange = async (c: Context<AuthEnv>, ready: BillingProvider) => {
    const row = await subscriptionFor(c);
    requirePayer(c, row);
    const change = await readBody(c, "SubscriptionChange");
    await refusePending(row);
    const subscription = heldSubscription(row, row.provider_customer_id);
    const paid = await ready.paid(subscription);
    const next = {
      product: change.product ?? paid.product,
      interval: change.interval ?? paid.interval,
      quantity: change.quantity ?? paid.quantity,
    };
    const from = product(paid.product);
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
    const same =
      next.product === paid.product &&
      next.interval === paid.interval &&
      next.quantity === paid.quantity;
    const when = change.when ?? "now";
    const held = row.next_product !== null;
    if (when === "now" && held && !same) throw heldForRenewal();
    const costsLess =
      next.quantity < paid.quantity ||
      (paid.interval === "year" && next.interval === "month") ||
      unit.amount < (unitPrice(from.id, next.interval)?.amount ?? 0);
    if (when === "now" && costsLess)
      throw new ApiFailure(
        409,
        "conflict",
        "A change that costs less is taken at renewal. Ask for it then.",
      );
    const orgId = to.id === "team" ? row.org_id : null;
    return { row, subscription, paid, next, unit, same, held, orgId, when };
  };

  const refuseFewerSeats = (held: number, quantity: number) => {
    if (quantity < held)
      throw new ApiFailure(
        409,
        "conflict",
        `${held} seats are held by members and pending invites; remove some before paying for fewer.`,
      );
  };

  const notNow = (why: NotNow) =>
    why === "held"
      ? heldForRenewal()
      : new ApiFailure(
          409,
          "conflict",
          why === "needs_renewal"
            ? "This payment method cannot be charged mid-period. Ask for the change at renewal."
            : "After a change was taken back, only seats change before renewal. Ask for this at renewal.",
        );

  app.post(
    "/subscriptions/:subscriptionId/preview",
    perUser(limiter, "billing-change", 20),
    async (c) => {
      const ready = needProvider(provider);
      const { subscription, next, unit, held, orgId, when } = await askedChange(
        c,
        ready,
      );
      if (held && when === "now") throw heldForRenewal();
      if (orgId) refuseFewerSeats(await seatsHeld(db, orgId), next.quantity);
      const preview = await ready.previewChange(
        subscription,
        { ...next, unit },
        when,
      );
      if ("refused" in preview) throw notNow(preview.refused);
      const body: ChangePreview = {
        now: preview.now,
        ...(preview.renewal
          ? {
              renewal: {
                at: preview.renewal.at.toISOString(),
                amount: preview.renewal.amount,
              },
            }
          : {}),
      };
      return c.json(body);
    },
  );

  app.patch(
    "/subscriptions/:subscriptionId",
    perUser(limiter, "billing-change", 20),
    async (c) => {
      const ready = needProvider(provider);
      const take = async () => {
        const asked = await askedChange(c, ready);
        const { row, subscription, paid, next, unit, same, orgId, when } =
          asked;
        const cutTo = (seats: number | null) =>
          db
            .updateTable("subscriptions")
            .set({ seats_cut_to: seats })
            .where("id", "=", row.id)
            .execute();
        if (same) {
          const dropped = await ready.takeBack(subscription, { ...next, unit });
          if (orgId && dropped) await cutTo(null);
          return asked;
        }
        const cut = next.quantity < paid.quantity ? next.quantity : null;
        if (orgId)
          await db.transaction().execute(async (trx) => {
            refuseFewerSeats(await lockSeats(trx, orgId), next.quantity);
            if (cut !== null)
              await trx
                .updateTable("subscriptions")
                .set({ seats_cut_to: cut })
                .where("id", "=", row.id)
                .execute();
          });
        let taken = false;
        try {
          const outcome = await ready.change(
            subscription,
            { ...next, unit },
            when,
          );
          if (!outcome.taken)
            throw outcome.why === "payment_failed"
              ? new ApiFailure(
                  409,
                  "conflict",
                  "The payment for the change did not go through, so nothing changed. Ask for it at renewal instead.",
                )
              : notNow(outcome.why);
          taken = true;
        } finally {
          if (orgId && cut !== null && !taken) await cutTo(row.seats_cut_to);
        }
        if (orgId && cut === null) await cutTo(null);
        return asked;
      };
      const { row, next, same, when } = await inTurn(
        c.req.param("subscriptionId") ?? "",
        take,
      );
      const { userId } = c.get("identity");
      await audit(db, {
        userId,
        actor: `user:${userId}`,
        action: same
          ? "billing.change_taken_back"
          : "billing.subscription_changed",
        subject: row.id,
        detail: same ? {} : { ...next, when },
      });
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
        if (!cancel) {
          requirePayer(c, row);
          await refusePending(row);
        }
        if (row.cancel_at_period_end !== cancel) {
          const subscription = heldSubscription(row, row.provider_customer_id);
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
    const body: InvoiceList = {
      invoices: await Promise.all(rows.map((row) => invoiceWithRefund(row))),
    };
    return c.json(body);
  });

  app.post(
    "/subscriptions/:subscriptionId/refund",
    perUser(limiter, "billing-refund", 5),
    async (c) => {
      const ready = needProvider(provider);
      const row = await subscriptionFor(c);
      await refundPlan(c, ready, row, row.id);
      return c.json(await reread(row.id));
    },
  );

  app.post(
    "/invoices/:invoiceId/refund",
    perUser(limiter, "billing-refund", 5),
    async (c) => {
      const ready = needProvider(provider);
      const row = await invoiceFor(c);
      if (!row.refund_requested_at) {
        const refund = await planRefund(
          db,
          ready.name,
          row.customer_id,
          row.provider_subscription_id,
        );
        if (
          !refund.requestedAt &&
          !refund.covers.some((invoice) => invoice.id === row.id)
        )
          throw new ApiFailure(
            409,
            "conflict",
            "A plan is refunded once, within its first days, and this payment is not part of that refund.",
          );
        await refundPlan(c, ready, row, row.id);
      }
      const after = await db
        .selectFrom("invoices")
        .selectAll()
        .where("id", "=", row.id)
        .executeTakeFirstOrThrow();
      return c.json(await invoiceWithRefund(after));
    },
  );

  app.get("/invoices/:invoiceId/receipt", async (c) => {
    const ready = needProvider(provider);
    const row = await invoiceFor(c);
    const receipt =
      row.provider === ready.name
        ? await ready.receipt(row.provider_payment_id)
        : null;
    if (!receipt)
      throw notFound("The provider has no receipt for this payment.");
    return c.json(receipt);
  });

  app.get("/portal", async (c) => {
    const ready = needProvider(provider);
    const payer = await payerOf(c, c.req.query("org"));
    const plan = await openPlanOf(payer, c.get("identity").userId, ready);
    const portal = plan ? await ready.portal(plan) : null;
    if (!portal)
      throw notFound("The provider has no page for this payer to manage.");
    return c.json(portal);
  });

  return app;
}

function invoiceBody(
  row: Selectable<Tables["invoices"]>,
  refundableUntil: Date | undefined,
): Invoice {
  return {
    id: row.id,
    lines: row.lines,
    reason: row.reason,
    total: { amount: Number(row.amount), currency: row.currency },
    status: row.status,
    at: row.at.toISOString(),
    ...(row.refund_requested_at && row.status !== "refunded"
      ? { refundRequestedAt: row.refund_requested_at.toISOString() }
      : {}),
    ...(refundableUntil
      ? { refundableUntil: refundableUntil.toISOString() }
      : {}),
  };
}
