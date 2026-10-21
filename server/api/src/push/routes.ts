import type { PushTokenState } from "@sikemux/protocol";
import { Hono } from "hono";
import { sql } from "kysely";

import { audit } from "../audit.ts";
import { allow, type AuthEnv } from "../auth.ts";
import type { Database } from "../db.ts";
import { proveKey } from "../devices/challenge.ts";
import { ownedDevice } from "../devices/owned.ts";
import { pushTokenMessage } from "../devices/signature.ts";
import { badRequest, readBody } from "../http.ts";
import { limit, type RateLimiter } from "../limits.ts";
import type { PushSettings } from "./settings.ts";

/** PUT and DELETE /v1/devices/{key}/push, mounted under the device routes, which sign the caller in. */
export function pushTokenRoutes(
  { db }: Database,
  limiter: RateLimiter,
  settings: Pick<PushSettings, "app" | "allowSandbox">,
) {
  const perUser = (name: string, perMinute: number) =>
    limit<AuthEnv>(limiter, name, perMinute, (c) => c.get("identity").userId);

  return new Hono<AuthEnv>()
    .use("/:key/push", allow("session"))
    .put("/:key/push", perUser("push-token", 20), async (c) => {
      const { userId } = c.get("identity");
      const key = c.req.param("key");
      const registration = await readBody(c, "PushTokenRegistration");
      if (
        (registration.platform === "apns") !==
        (registration.apnsEnvironment !== undefined)
      )
        throw badRequest(
          "APNs tokens say which environment issued them; FCM tokens do not.",
        );
      if (registration.app !== settings.app)
        throw badRequest(
          `This server delivers only to the ${settings.app} app.`,
        );
      if (registration.apnsEnvironment === "sandbox" && !settings.allowSandbox)
        throw badRequest(
          "This server does not deliver through Apple's sandbox.",
        );

      await proveKey(db, userId, {
        nonce: registration.nonce,
        key,
        message: pushTokenMessage(registration.nonce, key, registration.token),
        signature: registration.signature,
      });

      const updatedAt = await db.transaction().execute(async (trx) => {
        await ownedDevice(trx, userId, key, { role: "client", lock: true });

        await trx
          .deleteFrom("push_tokens")
          .where("token", "=", registration.token)
          .where("device_key", "<>", key)
          .execute();
        const fields = {
          platform: registration.platform,
          app: registration.app,
          apns_environment: registration.apnsEnvironment ?? null,
          token: registration.token,
        };
        const row = await trx
          .insertInto("push_tokens")
          .values({ device_key: key, ...fields })
          .onConflict((oc) =>
            oc.column("device_key").doUpdateSet({
              ...fields,
              updated_at: sql<Date>`now()`,
              last_ok_at: null,
              failures: 0,
            }),
          )
          .returning("updated_at")
          .executeTakeFirstOrThrow();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "push.enabled",
          subject: key,
          detail: { platform: registration.platform },
        });
        return row.updated_at;
      });
      c.get("log").info(
        { key: key.slice(0, 8), platform: registration.platform },
        "stored a push token",
      );
      const body: PushTokenState = {
        enabled: true,
        updatedAt: updatedAt.toISOString(),
      };
      return c.json(body);
    })
    .delete("/:key/push", perUser("push-token", 20), async (c) => {
      const { userId } = c.get("identity");
      const key = c.req.param("key");
      const cleared = await db.transaction().execute(async (trx) => {
        await ownedDevice(trx, userId, key, { role: "client" });
        const removed = await trx
          .deleteFrom("push_tokens")
          .where("device_key", "=", key)
          .executeTakeFirst();
        if (removed.numDeletedRows > 0n)
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "push.disabled",
            subject: key,
          });
        return removed.numDeletedRows > 0n;
      });
      if (cleared)
        c.get("log").info({ key: key.slice(0, 8) }, "cleared a push token");
      return c.body(null, 204);
    });
}
