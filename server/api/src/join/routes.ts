import { Hono } from "hono";

import { audit } from "../audit.ts";
import { isSession, type AuthEnv } from "../auth.ts";
import type { Database } from "../db.ts";
import { ownedDevice } from "../devices/owned.ts";
import { ApiFailure, readBody } from "../http.ts";
import { limit, type RateLimiter } from "../limits.ts";
import { signTicket, type JoinSigner } from "./signer.ts";

/**
 * POST /v1/devices/{key}/join, mounted under the device routes, which sign the caller in. The
 * phone app asks for a phone to join a host; the Mac app asks for one of the account's hosts to
 * link with another, which the ticket names in the phone's place.
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

  return new Hono<AuthEnv>().post("/:key/join", perJoiner, async (c) => {
    const identity = c.get("identity");
    const { userId } = identity;
    const joiner = c.req.param("key");
    const { host } = await readBody(c, "JoinRequest");
    const role = isSession(identity) ? "client" : "host";
    if (joiner === host)
      throw new ApiFailure(
        404,
        "not_found",
        "None of your hosts has that key.",
      );
    const device = await ownedDevice(
      db,
      userId,
      joiner,
      isSession(identity) ? { role } : {},
    );
    if (device.role !== role)
      throw new ApiFailure(
        403,
        "forbidden",
        "The Mac app may ask only for one of your hosts to link with another.",
      );
    await ownedDevice(db, userId, host, { role: "host" });

    const ticket = signTicket(signer, { account: userId, host, phone: joiner });
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
  });
}
