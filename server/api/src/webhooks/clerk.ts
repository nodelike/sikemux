import { createHmac, timingSafeEqual } from "node:crypto";
import { Hono } from "hono";

import { markDeleted } from "../account/deletion.ts";
import { audit } from "../audit.ts";
import type { Database } from "../db.ts";
import { ApiFailure, type Env, badRequest, unauthorized } from "../http.ts";

/** How far a webhook's timestamp may be from now, either way. */
const TOLERANCE_S = 5 * 60;
/** How long a handled message id is remembered, so a replay does nothing. */
const REMEMBER_MS = 24 * 60 * 60_000;

/** Whether Svix signed this body with the endpoint's secret, recently. */
export function verifySvix(
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
  data?: { id?: unknown };
}

/** Clerk's webhook. It acts only on user.deleted, cleaning up as if the person deleted the account here. */
export function clerkWebhookRoutes({ db }: Database, secret: string | null) {
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
    if (event.type !== "user.deleted" || typeof userId !== "string") {
      seen.remember(id);
      return c.body(null, 204);
    }
    if (!/^user_[A-Za-z0-9]+$/.test(userId))
      throw badRequest("The event names no Clerk user.");

    await markDeleted(db, {
      userId,
      actor: "clerk",
      via: "webhook",
      deletedInClerk: true,
    });
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
