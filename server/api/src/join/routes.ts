import { Hono } from "hono";

import { audit } from "../audit.ts";
import { allow, isSession, type AuthEnv } from "../auth.ts";
import type { Database } from "../db.ts";
import { ownedDevice } from "../devices/owned.ts";
import { notFound, readBody } from "../http.ts";
import { limit, type RateLimiter } from "../limits.ts";
import { signTicket, type JoinSigner } from "./signer.ts";

/**
 * POST /v1/devices/{key}/join, mounted under the device routes, which sign the caller in. The
 * phone app asks for a phone to join a host; a host, signing the request itself, asks to link
 * with another, which the ticket names in the phone's place, or for the app on it to join one as
 * a client. A ticket is of use only to the holder of the key it names.
 */
export function joinRoutes(
  { db }: Database,
  limiter: RateLimiter,
  signer: JoinSigner,
) {
  const perJoiner = limit<AuthEnv>(
    limiter,
    "join",
    10,
    (c) => `${c.get("identity").userId}:${c.req.param("key") ?? ""}`,
  );

  return new Hono<AuthEnv>().post(
    "/:key/join",
    allow("session", "host", "mac"),
    perJoiner,
    async (c) => {
      const identity = c.get("identity");
      const { userId } = identity;
      const joiner = c.req.param("key");
      const { host } = await readBody(c, "JoinRequest");
      const role =
        isSession(identity) ||
        (identity.via === "host" && joiner !== identity.deviceKey)
          ? "client"
          : "host";
      if (joiner === host) throw notFound("None of your hosts has that key.");
      const joining = await ownedDevice(db, userId, joiner, { role });
      await ownedDevice(db, userId, host, { role: "host" });
      if (
        identity.via === "host" &&
        role === "client" &&
        joining.registered_by !== identity.deviceKey
      )
        throw notFound("None of your devices has that key.");
      const via = joining.registered_by
        ? await ownedDevice(db, userId, joining.registered_by, {
            role: "host",
          })
        : undefined;

      const ticket = signTicket(signer, {
        account: userId,
        host,
        phone: joiner,
        ...(via && { via: { key: via.key, name: via.name } }),
      });
      await audit(db, {
        userId,
        actor: `user:${userId}`,
        action: "join.issued",
        subject: joiner,
        detail: { host, keyId: ticket.keyId },
      });
      c.get("log").info(
        { joiner: joiner.slice(0, 8), role, host: host.slice(0, 8) },
        "issued a join ticket",
      );
      return c.json(ticket);
    },
  );
}
