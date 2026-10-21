import { randomBytes } from "node:crypto";
import { Hono } from "hono";
import { sql } from "kysely";

import type { Database } from "../db.ts";
import { badRequest, type Env } from "../http.ts";
import { clientAddress, limit, type RateLimiter } from "../limits.ts";
import {
  CHANNELS,
  isChannel,
  isPlatform,
  PLATFORMS,
  RUNTIME_VERSION,
} from "./update.ts";

const RESPONSE_HEADERS = {
  "expo-protocol-version": "1",
  "expo-sfv-version": "0",
  "cache-control": "private, max-age=0",
};

/**
 * Expo's update protocol, version 1. Phones send headers, not JSON, so it sits outside /v1. Each
 * channel serves whatever was put on it last: an update, or a signed order to roll back.
 */
export function updateRoutes({ db }: Database, limiter: RateLimiter) {
  return new Hono<Env>().get(
    "/manifest",
    limit<Env>(limiter, "manifest", 120, clientAddress),
    async (c) => {
      const protocolVersion = c.req.header("expo-protocol-version");
      const platform = c.req.header("expo-platform");
      const runtimeVersion = c.req.header("expo-runtime-version");
      const channel = c.req.header("expo-channel-name");
      if (protocolVersion !== "1")
        throw badRequest("expo-protocol-version must be 1.");
      if (!isPlatform(platform))
        throw badRequest(`expo-platform is one of ${PLATFORMS.join(", ")}.`);
      if (!runtimeVersion || !RUNTIME_VERSION.test(runtimeVersion))
        throw badRequest("expo-runtime-version is missing or malformed.");
      if (!isChannel(channel))
        throw badRequest(`expo-channel-name is one of ${CHANNELS.join(", ")}.`);

      const part = await db
        .selectFrom("update_rollbacks")
        .select([
          sql<string>`'directive'`.as("name"),
          "directive as body",
          "signature",
          "assigned_at",
        ])
        .where("platform", "=", platform)
        .where("runtime_version", "=", runtimeVersion)
        .where("channel", "=", channel)
        .unionAll(
          db
            .selectFrom("updates")
            .innerJoin(
              "update_channels",
              "update_channels.update_id",
              "updates.id",
            )
            .select([
              sql<string>`'manifest'`.as("name"),
              "updates.manifest as body",
              "updates.signature",
              "update_channels.assigned_at",
            ])
            .where("updates.platform", "=", platform)
            .where("updates.runtime_version", "=", runtimeVersion)
            .where("update_channels.channel", "=", channel)
            .where("update_channels.withdrawn_at", "is", null),
        )
        .orderBy("assigned_at", "desc")
        .orderBy("name")
        .limit(1)
        .executeTakeFirst();

      for (const [name, value] of Object.entries(RESPONSE_HEADERS))
        c.header(name, value);
      // Protocol 1 reads an empty 204 as "nothing new", which needs no signature.
      if (!part) return c.body(null, 204);

      const boundary = boundaryFor(part.body);
      const head = Buffer.from(
        [
          `--${boundary}`,
          "content-type: application/json; charset=utf-8",
          `content-disposition: form-data; name="${part.name}"`,
          `expo-signature: sig="${part.signature}", keyid="main", alg="rsa-v1_5-sha256"`,
          "",
          "",
        ].join("\r\n"),
      );
      const tail = Buffer.from(`\r\n--${boundary}--\r\n`);
      c.header("content-type", `multipart/mixed; boundary=${boundary}`);
      return c.body(new Uint8Array(Buffer.concat([head, part.body, tail])));
    },
  );
}

function boundaryFor(body: Buffer): string {
  for (;;) {
    const boundary = `sikemux-${randomBytes(16).toString("hex")}`;
    if (!body.includes(boundary)) return boundary;
  }
}
