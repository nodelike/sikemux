import { createHash, timingSafeEqual } from "node:crypto";
import { Hono } from "hono";
import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Env } from "../http.ts";
import type { AccountWatcher } from "../live/listener.ts";

/** The header iroh-relay names the connecting device in. */
const ENDPOINT_HEADER = "x-iroh-nodeid";
const REMEMBER_MS = 60_000;
const MAX_REMEMBERED = 50_000;

function sameSecret(given: string, expected: string): boolean {
  const digest = (text: string) => createHash("sha256").update(text).digest();
  return timingSafeEqual(digest(given), digest(expected));
}

/**
 * Which devices may use the relay. A yes is remembered for a minute, so a device reconnecting
 * costs no query, and is forgotten as soon as its account changes.
 */
export class RelayAccess implements AccountWatcher {
  private readonly allowed = new Map<
    string,
    { userId: string; until: number }
  >();
  private readonly db: Kysely<Tables>;
  private readonly now: () => number;

  constructor(db: Kysely<Tables>, now: () => number = Date.now) {
    this.db = db;
    this.now = now;
  }

  async allows(key: string): Promise<boolean> {
    const now = this.now();
    const remembered = this.allowed.get(key);
    if (remembered && remembered.until > now) return true;
    const device = await this.db
      .selectFrom("devices")
      .innerJoin("users", "users.id", "devices.user_id")
      .select("devices.user_id")
      .where("devices.key", "=", key)
      .where("users.deleted_at", "is", null)
      .executeTakeFirst();
    if (!device) {
      this.allowed.delete(key);
      return false;
    }
    if (this.allowed.size >= MAX_REMEMBERED) this.sweep(now);
    this.allowed.set(key, { userId: device.user_id, until: now + REMEMBER_MS });
    return true;
  }

  changed(userId: string) {
    for (const [key, entry] of this.allowed)
      if (entry.userId === userId) this.allowed.delete(key);
  }

  missed() {
    this.allowed.clear();
  }

  private sweep(now: number) {
    for (const [key, entry] of this.allowed)
      if (entry.until <= now) this.allowed.delete(key);
    if (this.allowed.size >= MAX_REMEMBERED) this.allowed.clear();
  }
}

/**
 * POST /internal/relay-access, which relay.sikemux.com asks before it carries a device's
 * traffic: only devices on an account that stands may use it. Caddy keeps /internal off the
 * internet, and the relay proves itself with the token both are given.
 */
export function relayAccessRoutes(access: RelayAccess, token: string | null) {
  return new Hono<Env>().post("/relay-access", async (c) => {
    const bearer = /^Bearer\s+(\S+)$/i.exec(
      c.req.header("authorization") ?? "",
    );
    if (!token || !bearer?.[1] || !sameSecret(bearer[1], token))
      return c.text("false", 401);
    const endpoint = c.req.header(ENDPOINT_HEADER) ?? "";
    if (!/^[0-9a-f]{64}$/.test(endpoint)) return c.text("false", 403);
    return (await access.allows(endpoint))
      ? c.text("true")
      : c.text("false", 403);
  });
}
