import {
  deviceRoleValues,
  type DeviceList,
  type DeviceRole,
} from "@sikemux/protocol";
import { Hono } from "hono";
import { sql } from "kysely";

import type { ClerkBackend } from "../account/clerk.ts";
import { revokeRemovedSession } from "../account/deletion.ts";
import {
  allow,
  requireIdentity,
  type AuthEnv,
  type Verifier,
} from "../auth.ts";
import { audit } from "../audit.ts";
import type { Database } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { badRequest, readBody } from "../http.ts";
import { joinRoutes } from "../join/routes.ts";
import type { JoinSigner } from "../join/signer.ts";
import { limit, type RateLimiter } from "../limits.ts";
import { pushTokenRoutes } from "../push/routes.ts";
import type { PushSettings } from "../push/settings.ts";
import { issueChallenge } from "./challenge.ts";
import { ownedDevice } from "./owned.ts";
import { registerDevice, toDevice } from "./register.ts";
import { removeDevice } from "./removal.ts";

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

  return (
    new Hono<AuthEnv>()
      .use(
        requireIdentity(verifier, db, {
          accept: ["session", "host", "macRegistration"],
        }),
      )
      .route("/", pushTokenRoutes(database, limiter, push))
      .route("/", joinRoutes(database, limiter, join))
      .post(
        "/challenge",
        allow("session", "macRegistration", "host"),
        perUser("challenge", 30),
        async (c) => c.json(await issueChallenge(db, c.get("identity").userId)),
      )
      .post(
        "/",
        allow("session", "macRegistration", "host"),
        perUser("register", 20),
        async (c) => {
          const registration = await readBody(c, "DeviceRegistration");
          const { row, added } = await registerDevice(
            db,
            c.get("identity"),
            registration,
          );
          c.get("log").info(
            { key: row.key, role: row.role, added },
            added ? "registered a device" : "updated a device",
          );
          return c.json(toDevice(row), added ? 201 : 200);
        },
      )
      // A host signs this only for the person in its own app, as it signs every request it makes
      // here (the core refuses to sign for any other program), so it stands for the person: ADR 0018
      // lets any device on the account take another off it. A host leaves by signing out instead.
      .delete(
        "/:key",
        allow("session", "host"),
        perUser("remove", 30),
        async (c) => {
          const { userId, via, sessionId, deviceKey } = c.get("identity");
          const key = c.req.param("key");
          if (key === deviceKey)
            throw badRequest("A host leaves the account by signing out.");
          const { device, reason } = await db
            .transaction()
            .execute(async (trx) => {
              const device = await ownedDevice(trx, userId, key, {
                lock: true,
              });
              const reason =
                sessionId !== undefined && device.clerk_session_id === sessionId
                  ? ("signed_out" as const)
                  : ("removed" as const);
              await removeDevice(trx, {
                userId,
                key,
                reason,
                actor: `user:${userId}`,
                detail: { via },
              });
              return { device, reason };
            });
          c.get("log").info(
            { key, role: device.role, reason },
            "removed a device",
          );
          if (reason === "removed" && device.clerk_session_id)
            await revokeRemovedSession(db, clerk, c.get("log"), key).catch(
              (error: unknown) =>
                c
                  .get("log")
                  .warn({ err: error }, "could not revoke the session"),
            );
          return c.body(null, 204);
        },
      )
      // Hosts sign this only for the person in their own app, as for removing a device.
      .post(
        "/:key/name",
        allow("session", "host"),
        perUser("rename", 30),
        async (c) => {
          const { userId, via } = c.get("identity");
          const key = c.req.param("key");
          const name = (await readBody(c, "DeviceRename")).name.trim();
          if (!name) throw badRequest("The device needs a name.");
          const row = await db.transaction().execute(async (trx) => {
            const device = await ownedDevice(trx, userId, key, { lock: true });
            await appendEvent(trx, {
              userId,
              type: "device.changed",
              subject: key,
              subjectRole: device.role as DeviceRole,
            });
            await audit(trx, {
              userId,
              actor: `user:${userId}`,
              action: "device.renamed",
              subject: key,
              detail: { from: device.name, to: name, via },
            });
            return trx
              .updateTable("devices")
              .set({ name, updated_at: sql<Date>`now()` })
              .where("key", "=", key)
              .returningAll()
              .executeTakeFirstOrThrow();
          });
          return c.json(toDevice(row));
        },
      )
      .get(
        "/",
        allow("session", "host", "mac"),
        perUser("list", 120),
        async (c) => {
          const { userId } = c.get("identity");
          const role = c.req.query("role");
          if (
            role !== undefined &&
            !deviceRoleValues.includes(role as DeviceRole)
          ) {
            throw badRequest(`role is one of ${deviceRoleValues.join(", ")}.`);
          }
          let query = db
            .selectFrom("devices")
            .selectAll()
            .where("user_id", "=", userId);
          if (role) query = query.where("role", "=", role);
          const rows = await query.orderBy("created_at", "desc").execute();
          const body: DeviceList = { devices: rows.map(toDevice) };
          return c.json(body);
        },
      )
  );
}
