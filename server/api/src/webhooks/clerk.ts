import { createHmac, timingSafeEqual } from "node:crypto";
import { Hono } from "hono";

import { signsIn, type ClerkBackend } from "../account/clerk.ts";
import type { ReviewAccount } from "../account/review-sign-in.ts";
import { noteAddresses, stripWorkSignIns } from "../account/sign-ins.ts";
import { markDeleted } from "../account/deletion.ts";
import { audit } from "../audit.ts";
import { followAddress } from "../billing/customers.ts";
import { endDeletedBilling } from "../billing/deleted.ts";
import type { BillingProvider } from "../billing/provider.ts";
import type { Database } from "../db.ts";
import { ApiFailure, type Env, badRequest, unauthorized } from "../http.ts";

/** How far a webhook's timestamp may be from now, either way. */
const TOLERANCE_S = 5 * 60;
/** How long a handled message id is remembered, so a replay does nothing. */
const REMEMBER_MS = 24 * 60 * 60_000;

/** Whether Svix signed this body with the endpoint's secret, recently. */
function verifySvix(
  secret: string,
  headers: {
    id: string | undefined;
    timestamp: string | undefined;
    signature: string | undefined;
  },
  body: string,
  nowS = Math.floor(Date.now() / 1000),
): boolean {
  const { id, timestamp, signature } = headers;
  if (!id || !timestamp || !signature || !/^\d+$/.test(timestamp)) return false;
  if (Math.abs(nowS - Number(timestamp)) > TOLERANCE_S) return false;
  const key = Buffer.from(secret.replace(/^whsec_/, ""), "base64");
  const expected = createHmac("sha256", key)
    .update(`${id}.${timestamp}.${body}`)
    .digest();
  return signature.split(" ").some((entry) => {
    const [version, value] = entry.split(",");
    if (version !== "v1" || !value) return false;
    const given = Buffer.from(value, "base64");
    return given.length === expected.length && timingSafeEqual(given, expected);
  });
}

class SeenIds {
  private readonly seen = new Map<string, number>();

  has(id: string, now = Date.now()): boolean {
    for (const [known, at] of this.seen) {
      if (now - at < REMEMBER_MS) break;
      this.seen.delete(known);
    }
    return this.seen.has(id);
  }

  remember(id: string, now = Date.now()) {
    this.seen.set(id, now);
  }
}

interface ClerkEvent {
  type?: unknown;
  data?: {
    id?: unknown;
    email_addresses?: {
      email_address?: unknown;
      created_at?: unknown;
      reserved?: boolean;
      verification?: { status?: string } | null;
    }[];
    external_accounts?: { email_address?: unknown; created_at?: unknown }[];
  };
}

/** The addresses a user.updated event shows, its connected accounts' too, each with when Clerk says it was added. */
function shownAddresses(event: ClerkEvent) {
  const listed = (list: unknown) =>
    (Array.isArray(list) ? list : []) as NonNullable<
      NonNullable<ClerkEvent["data"]>["email_addresses"]
    >;
  return [
    ...listed(event.data?.email_addresses).map((address) => ({
      address,
      signsIn: signsIn(address),
    })),
    ...listed(event.data?.external_accounts).map((address) => ({
      address,
      signsIn: true,
    })),
  ].flatMap(({ address, signsIn }) =>
    typeof address.email_address === "string" &&
    typeof address.created_at === "number"
      ? [
          {
            email: address.email_address,
            addedAt: new Date(address.created_at),
            signsIn,
          },
        ]
      : [],
  );
}

/**
 * Clerk's webhook. On user.deleted it cleans up as if the person deleted the account here; on
 * user.updated it moves their payment customer to the email they now sign in with.
 */
export function clerkWebhookRoutes(
  { db }: Database,
  secret: string | null,
  provider: BillingProvider | null,
  clerk: ClerkBackend | null,
  proofKey: Buffer | null,
  review: ReviewAccount | null,
) {
  const seen = new SeenIds();
  return new Hono<Env>().post("/clerk", async (c) => {
    if (!secret)
      throw new ApiFailure(
        503,
        "unavailable",
        "CLERK_WEBHOOK_SECRET is not set, so no webhook is accepted.",
      );
    const body = await c.req.text();
    const id = c.req.header("svix-id");
    const signed = verifySvix(
      secret,
      {
        id,
        timestamp: c.req.header("svix-timestamp"),
        signature: c.req.header("svix-signature"),
      },
      body,
    );
    if (!signed || !id)
      throw unauthorized("The webhook signature is not valid.");
    if (seen.has(id)) return c.body(null, 204);

    let event: ClerkEvent;
    try {
      event = JSON.parse(body) as ClerkEvent;
    } catch {
      throw badRequest("The body is not JSON.");
    }
    const userId = event.data?.id;
    const handled = ["user.deleted", "user.updated"];
    if (
      typeof event.type !== "string" ||
      !handled.includes(event.type) ||
      typeof userId !== "string"
    ) {
      seen.remember(id);
      return c.body(null, 204);
    }
    if (!/^user_[A-Za-z0-9]+$/.test(userId))
      throw badRequest("The event names no Clerk user.");
    if (review && clerk && userId === review.userId)
      await review
        .load(clerk, c.get("log"))
        .catch((error: unknown) =>
          c
            .get("log")
            .warn({ err: error }, "reading the demo account from Clerk failed"),
        );

    if (event.type === "user.updated") {
      await noteAddresses(db, proofKey, userId, shownAddresses(event));
      if (clerk)
        await stripWorkSignIns(db, clerk, proofKey, c.get("log"), userId);
      if (provider && clerk)
        await followAddress(db, provider, clerk, c.get("log"), userId);
      seen.remember(id);
      return c.body(null, 204);
    }

    await markDeleted(db, {
      userId,
      actor: "clerk",
      via: "webhook",
      deletedInClerk: true,
    });
    await endDeletedBilling(db, provider, c.get("log"), userId);
    await audit(db, {
      userId,
      actor: "clerk",
      action: "webhook.user_deleted",
      subject: id,
    });
    seen.remember(id);
    c.get("log").info({ userId }, "Clerk deleted a user");
    return c.body(null, 204);
  });
}
