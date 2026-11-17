import type { Services } from "../app.ts";
import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import type { Hook, HookAccepted, HookHeader } from "@sikemux/protocol";
import { hookMethodValues } from "@sikemux/protocol";
import { Hono, type Context } from "hono";
import { bodyLimit } from "hono/body-limit";
import { sql } from "kysely";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import {
  ApiFailure,
  badRequest,
  errorResponse,
  notFound,
  oneOf,
  readBody,
  type Env,
} from "../http.ts";
import { HOOK_OPTIONS } from "./options.ts";

const HOOK_ID = /^[0-9a-f]{32}$/;
const TOKEN = /^[0-9a-f]{64}$/;
const MAX_QUERY = 8192;
const MAX_HEADERS = 200;

/** Headers the connection or the proxy in front of the API added, which say nothing of the sender's request. */
const DROPPED_HEADERS = new Set([
  "connection",
  "content-length",
  "forwarded",
  "host",
  "keep-alive",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
  "via",
  "x-real-ip",
]);

function kept(name: string): boolean {
  return (
    !DROPPED_HEADERS.has(name) &&
    !name.startsWith("x-forwarded-") &&
    !name.startsWith("proxy-")
  );
}

function sha256(text: string): Buffer {
  return createHash("sha256").update(text).digest();
}

function hostKey(c: Context<AuthEnv>): string {
  const key = c.get("identity").deviceKey;
  if (!key) throw new ApiFailure(403, "forbidden", "Only a host has webhooks.");
  return key;
}

/** Hosts adding and removing the webhook addresses that reach them wherever they are. */
export function hookRoutes({
  database: { db },
  verifier,
  hookHosts: hosts,
  apiOrigin,
}: Pick<Services, "database" | "verifier" | "hookHosts" | "apiOrigin">) {
  const asHost = requireIdentity(verifier, db, { accept: ["host"] });
  const app = new Hono<AuthEnv>();

  app.post("/", asHost, async (c) => {
    const key = hostKey(c);
    const { token } = await readBody(c, "HookAdd");
    const hook = await db.transaction().execute(async (trx) => {
      await trx
        .selectFrom("devices")
        .select("key")
        .where("key", "=", key)
        .forUpdate()
        .execute();
      const held = await trx
        .selectFrom("hooks")
        .select((eb) => eb.fn.countAll<string>().as("count"))
        .where("host_key", "=", key)
        .executeTakeFirstOrThrow();
      if (Number(held.count) >= HOOK_OPTIONS.hooksPerHost)
        throw new ApiFailure(
          409,
          "conflict",
          `A host may have ${HOOK_OPTIONS.hooksPerHost} webhooks.`,
        );
      return trx
        .insertInto("hooks")
        .values({
          id: randomBytes(16).toString("hex"),
          user_id: c.get("identity").userId,
          host_key: key,
          token_sha256: sha256(token),
        })
        .returning(["id", "created_at"])
        .executeTakeFirstOrThrow();
    });
    const body: Hook = {
      id: hook.id,
      url: `${apiOrigin}/v1/hooks/${hook.id}/${token}`,
      createdAt: hook.created_at.toISOString(),
    };
    return c.json(body, 201);
  });

  app.delete("/:hookId", asHost, async (c) => {
    const key = hostKey(c);
    await db
      .deleteFrom("hooks")
      .where("id", "=", c.req.param("hookId"))
      .where("host_key", "=", key)
      .execute();
    hosts.checkIdle(key);
    return c.body(null, 204);
  });

  return app;
}

/**
 * What senders post to a webhook. The request waits here, for a day at most, until its host's
 * connection takes it. Each webhook has its own limit, since one sender's addresses serve many
 * accounts.
 */
export function hookReceiveRoutes({
  database: { db },
  limiter,
  hookHosts: hosts,
}: Pick<Services, "database" | "limiter" | "hookHosts">) {
  const receive = new Hono<Env>();
  receive.on(
    hookMethodValues.map((method) => method.toUpperCase()),
    "/:hookId/:token",
    bodyLimit({
      maxSize: HOOK_OPTIONS.maxBodyBytes,
      onError: (c) =>
        errorResponse(
          c,
          413,
          "payload_too_large",
          `A webhook takes bodies of at most ${HOOK_OPTIONS.maxBodyBytes} bytes.`,
        ),
    }),
    async (c) => {
      const id = c.req.param("hookId");
      const token = c.req.param("token");
      const unknown = () => notFound("No webhook has this address.");
      if (!HOOK_ID.test(id) || !TOKEN.test(token)) throw unknown();
      if (!limiter.allow(`hook:${id}`, HOOK_OPTIONS.requestsPerMinute)) {
        c.header("retry-after", "60");
        throw new ApiFailure(
          429,
          "rate_limited",
          `This webhook takes at most ${HOOK_OPTIONS.requestsPerMinute} requests a minute.`,
        );
      }
      const hook = await db
        .selectFrom("hooks")
        .select(["host_key", "token_sha256"])
        .where("id", "=", id)
        .executeTakeFirst();
      if (!hook || !timingSafeEqual(hook.token_sha256, sha256(token)))
        throw unknown();

      const query = new URL(c.req.url).search.slice(1);
      if (query.length > MAX_QUERY)
        throw badRequest(
          `A webhook takes a query of at most ${MAX_QUERY} characters.`,
        );
      const headers: HookHeader[] = [];
      c.req.raw.headers.forEach((value, name) => {
        if (kept(name)) headers.push({ name, value });
      });
      if (headers.length > MAX_HEADERS)
        throw badRequest(`A webhook takes at most ${MAX_HEADERS} headers.`);
      const body = Buffer.from(await c.req.arrayBuffer());

      const delivery = await db.transaction().execute(async (trx) => {
        await trx
          .selectFrom("devices")
          .select("key")
          .where("key", "=", hook.host_key)
          .forUpdate()
          .execute();
        const waiting = await trx
          .selectFrom("hook_deliveries")
          .select((eb) => [
            eb.fn.countAll<string>().as("count"),
            sql<string>`coalesce(sum(length(body)), 0)`.as("bytes"),
          ])
          .where("host_key", "=", hook.host_key)
          .executeTakeFirstOrThrow();
        if (
          Number(waiting.count) >= HOOK_OPTIONS.waitingPerHost ||
          Number(waiting.bytes) + body.length > HOOK_OPTIONS.waitingBytesPerHost
        )
          throw new ApiFailure(
            429,
            "rate_limited",
            "The webhook's host has as many requests waiting as it may hold.",
          );
        return trx
          .insertInto("hook_deliveries")
          .values({
            hook_id: id,
            host_key: hook.host_key,
            method: oneOf(
              "method",
              c.req.method.toLowerCase(),
              hookMethodValues,
            ),
            headers: JSON.stringify(headers),
            query,
            body,
          })
          .returning("id")
          .executeTakeFirstOrThrow();
      });
      hosts.wake(hook.host_key);
      const accepted: HookAccepted = { id: delivery.id };
      return c.json(accepted, 202);
    },
  );
  return receive;
}
