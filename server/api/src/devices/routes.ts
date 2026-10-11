import { randomBytes } from "node:crypto";
import type {
  Challenge,
  Channel,
  Device,
  DeviceList,
  DeviceRole,
  Platform,
} from "@sikemux/protocol";
import { Hono } from "hono";
import { sql, type Selectable } from "kysely";

import type { ClerkBackend } from "../account/clerk.ts";
import { revokeRemovedSession } from "../account/deletion.ts";
import { requireIdentity, type AuthEnv, type Verifier } from "../auth.ts";
import type { Database, Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, readBody } from "../http.ts";
import { joinRoutes } from "../join/routes.ts";
import type { JoinSigner } from "../join/signer.ts";
import { limit, type RateLimiter } from "../limits.ts";
import { pushTokenRoutes } from "../push/routes.ts";
import type { PushSettings } from "../push/settings.ts";
import { removeDevice } from "./removal.ts";
import { registrationMessage, signedBy } from "./signature.ts";

const CHALLENGE_MS = 2 * 60_000;
const ROLES: readonly DeviceRole[] = ["host", "client"];

function toDevice(row: Selectable<Tables["devices"]>): Device {
  const device: Device = {
    key: row.key,
    role: row.role as DeviceRole,
    name: row.name,
    platform: row.platform as Platform,
    createdAt: row.created_at.toISOString(),
    lastSeenAt: row.last_seen_at?.toISOString() ?? null,
  };
  if (row.channel) device.channel = row.channel as Channel;
  return device;
}

export function deviceRoutes(
  database: Database,
  verifier: Verifier,
  limiter: RateLimiter,
  clerk: ClerkBackend | null,
  push: Pick<PushSettings, "app" | "allowSandbox">,
  join: JoinSigner,
) {
  const { db } = database;
  const perUser = (name: string, perMinute: number) =>
    limit<AuthEnv>(limiter, name, perMinute, (c) => c.get("identity").userId);

  return new Hono<AuthEnv>()
    .use(requireIdentity(verifier, db))
    .route("/", pushTokenRoutes(database, limiter, push))
    .route("/", joinRoutes(database, limiter, join))
    .post("/challenge", perUser("challenge", 30), async (c) => {
      const { userId } = c.get("identity");
      const nonce = randomBytes(32).toString("hex");
      const expiresAt = new Date(Date.now() + CHALLENGE_MS);
      await db
        .deleteFrom("challenges")
        .where("expires_at", "<", sql<Date>`now() - interval '1 hour'`)
        .execute();
      await db
        .insertInto("challenges")
        .values({ nonce, user_id: userId, expires_at: expiresAt })
        .execute();
      const body: Challenge = { nonce, expiresAt: expiresAt.toISOString() };
      return c.json(body);
    })
    .post("/", perUser("register", 20), async (c) => {
      const { userId } = c.get("identity");
      const registration = await readBody(c, "DeviceRegistration");
      if (
        (registration.role === "host") !==
        (registration.channel !== undefined)
      ) {
        throw new ApiFailure(
          400,
          "bad_request",
          "Hosts say which channel they run; clients do not.",
        );
      }
      const name = registration.name.trim();
      if (!name)
        throw new ApiFailure(400, "bad_request", "The device needs a name.");

      const challenge = await db
        .deleteFrom("challenges")
        .where("nonce", "=", registration.nonce)
        .where("user_id", "=", userId)
        .where("expires_at", ">", sql<Date>`now()`)
        .returning("nonce")
        .executeTakeFirst();
      if (!challenge) {
        throw new ApiFailure(
          400,
          "bad_request",
          "The challenge is unknown, already used, or expired.",
        );
      }
      const message = registrationMessage(
        registration.nonce,
        userId,
        registration.key,
      );
      if (!signedBy(registration.key, message, registration.signature)) {
        throw new ApiFailure(
          403,
          "forbidden",
          "The signature does not prove this device holds the key.",
        );
      }

      const identity = c.get("identity");
      const { row, added } = await db.transaction().execute(async (trx) => {
        await trx
          .insertInto("users")
          .values({ id: userId })
          .onConflict((oc) => oc.column("id").doNothing())
          .execute();
        const user = await trx
          .selectFrom("users")
          .select("deleted_at")
          .where("id", "=", userId)
          .forShare()
          .executeTakeFirstOrThrow();
        if (user.deleted_at)
          throw new ApiFailure(
            401,
            "unauthorized",
            "This account was deleted. Sign in to the phone app or app.sikemux.com to restore it.",
          );
        const existing = await trx
          .selectFrom("devices")
          .selectAll()
          .where("key", "=", registration.key)
          .forUpdate()
          .executeTakeFirst();
        if (existing && existing.user_id !== userId) {
          throw new ApiFailure(
            409,
            "conflict",
            "This device is registered to another account.",
          );
        }
        if (existing && existing.role !== registration.role) {
          throw new ApiFailure(
            409,
            "conflict",
            `This device is already registered as a ${existing.role}.`,
          );
        }
        const fields = {
          name,
          platform: registration.platform,
          channel: registration.channel ?? null,
          clerk_session_id:
            registration.role === "client"
              ? (identity.sessionId ?? null)
              : null,
        };
        const eventId = await appendEvent(trx, {
          userId,
          type: existing ? "device.changed" : "device.added",
          subject: registration.key,
          subjectRole: registration.role,
        });
        const row = existing
          ? await trx
              .updateTable("devices")
              .set({ ...fields, updated_at: sql<Date>`now()` })
              .where("key", "=", registration.key)
              .returningAll()
              .executeTakeFirstOrThrow()
          : await trx
              .insertInto("devices")
              .values({
                key: registration.key,
                user_id: userId,
                role: registration.role,
                acked_event_id: eventId,
                ...fields,
              })
              .returningAll()
              .executeTakeFirstOrThrow();
        if (!existing)
          await trx
            .deleteFrom("removed_devices")
            .where("key", "=", registration.key)
            .execute();
        await trx
          .insertInto("audit")
          .values({
            user_id: userId,
            actor: `user:${userId}`,
            action: existing ? "device.updated" : "device.registered",
            subject: registration.key,
            detail: JSON.stringify({
              role: row.role,
              name: row.name,
              platform: row.platform,
              via: identity.via,
            }),
          })
          .execute();
        return { row, added: !existing };
      });
      c.get("log").info(
        { key: row.key, role: row.role, added },
        added ? "registered a device" : "updated a device",
      );
      return c.json(toDevice(row), added ? 201 : 200);
    })
    .delete("/:key", perUser("remove", 30), async (c) => {
      const { userId, via, sessionId } = c.get("identity");
      const key = c.req.param("key");
      const { removed, reason } = await db
        .transaction()
        .execute(async (trx) => {
          const device = await trx
            .selectFrom("devices")
            .select("clerk_session_id")
            .where("key", "=", key)
            .where("user_id", "=", userId)
            .forUpdate()
            .executeTakeFirst();
          const reason =
            sessionId !== undefined && device?.clerk_session_id === sessionId
              ? ("signed_out" as const)
              : ("removed" as const);
          const removed = device
            ? await removeDevice(trx, {
                userId,
                key,
                reason,
                actor: `user:${userId}`,
                detail: { via },
              })
            : undefined;
          return { removed, reason };
        });
      if (!removed)
        throw new ApiFailure(
          404,
          "not_found",
          "None of your devices has that key.",
        );
      c.get("log").info(
        { key, role: removed.role, reason },
        "removed a device",
      );
      if (reason === "removed" && removed.clerkSessionId)
        await revokeRemovedSession(db, clerk, c.get("log"), key).catch(
          (error: unknown) =>
            c.get("log").warn({ err: error }, "could not revoke the session"),
        );
      return c.body(null, 204);
    })
    .get("/", perUser("list", 120), async (c) => {
      const { userId } = c.get("identity");
      const role = c.req.query("role");
      if (role !== undefined && !ROLES.includes(role as DeviceRole)) {
        throw new ApiFailure(
          400,
          "bad_request",
          `role is one of ${ROLES.join(", ")}.`,
        );
      }
      let query = db
        .selectFrom("devices")
        .selectAll()
        .where("user_id", "=", userId);
      if (role) query = query.where("role", "=", role);
      const rows = await query.orderBy("created_at", "desc").execute();
      const body: DeviceList = { devices: rows.map(toDevice) };
      return c.json(body);
    });
}
