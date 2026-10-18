import { randomBytes } from "node:crypto";
import type {
  ShareClientMessage,
  ShareEndReason,
  ShareHello,
  ShareOpen,
  ShareServerMessage,
} from "@sikemux/protocol";
import { ShareCloseCode, validator } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";
import type { RawData, WebSocket } from "ws";

import { checkStanding, type Verifier } from "../auth.ts";
import type { Tables } from "../db.ts";
import { shareMessage, signedBy } from "../devices/signature.ts";
import { ApiFailure } from "../http.ts";
import type { RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";
import type { ShareOptions } from "./options.ts";
import { Room, type HostLink, type Relay, type ViewerLink } from "./relay.ts";

export interface ShareContext {
  db: Kysely<Tables>;
  verifier: Verifier;
  relay: Relay;
  limiter: RateLimiter;
  log: Logger;
  options: ShareOptions;
}

class HelloRefused extends Error {}

const protocol = validator();

export function failedShareHelloKeys(ip: string, key?: string): string[] {
  return [`share-failed-ip:${ip}`, ...(key ? [`share-failed-key:${key}`] : [])];
}

/** Marks the share ended in the database and tells everyone on it. */
export async function endShare(
  context: Pick<ShareContext, "db" | "relay" | "log">,
  id: string,
  reason: ShareEndReason,
) {
  await context.db
    .updateTable("shares")
    .set({ ended_at: sql<Date>`now()`, ended_reason: reason })
    .where("id", "=", id)
    .where("ended_at", "is", null)
    .execute();
  const room = context.relay.rooms.get(id);
  context.relay.rooms.delete(id);
  room?.end(reason);
  context.log.info({ share: id, reason }, "a share ended");
}

/** The room for an open share, made from its row when this process has not held it yet. */
function roomFor(
  context: ShareContext,
  row: { id: string; host_key: string; expires_at: Date },
): Room {
  const { relay } = context;
  let room = relay.rooms.get(row.id);
  if (!room) {
    room = new Room(row.id, row.host_key, row.expires_at, (expired) => {
      endShare(context, expired.id, "expired").catch((error: unknown) =>
        context.log.error({ err: error }, "ending an expired share failed"),
      );
    });
    relay.rooms.set(row.id, room);
    const host = relay.hosts.get(row.host_key);
    if (host) room.attachHost(host);
  }
  return room;
}

/**
 * One connection to /v1/share: a host publishing its shared chats, or a viewer watching one.
 * Viewers only ever read; what they ask, such as an older page, goes sealed to the host, which
 * decides what to answer.
 */
export class ShareSocket implements HostLink, ViewerLink {
  readonly id = randomBytes(9).toString("base64url");
  key = "";
  private userId = "";
  private role: "host" | "viewer" | null = null;
  private phase: "hello" | "proving" | "open" | "closing" = "hello";
  private room: Room | null = null;
  private readonly nonce = randomBytes(32).toString("hex");
  private readonly challengeExpires: number;
  private lastHeard = Date.now();
  private messageWindow = { started: 0, count: 0 };
  private queue: Promise<void> = Promise.resolve();
  private readonly timers: NodeJS.Timeout[] = [];
  private authTimer: NodeJS.Timeout | undefined;
  private readonly log: Logger;

  private readonly ws: WebSocket;
  private readonly ip: string;
  private readonly context: ShareContext;

  constructor(ws: WebSocket, ip: string, context: ShareContext) {
    this.ws = ws;
    this.ip = ip;
    this.context = context;
    this.challengeExpires = Date.now() + context.options.challengeMs;
    this.log = context.log.child({ share: true });
  }

  start() {
    const { options, relay } = this.context;
    relay.sockets += 1;
    this.ws.on("message", (data, isBinary) => this.onMessage(data, isBinary));
    this.ws.on("close", () => this.onClose());
    this.ws.on("error", (error) =>
      this.log.debug({ err: error }, "a share connection failed"),
    );
    this.send({
      type: "challenge",
      nonce: this.nonce,
      expiresAt: new Date(this.challengeExpires).toISOString(),
    });
    this.timers.push(
      setTimeout(() => {
        if (this.phase === "hello")
          this.close(ShareCloseCode.helloTimeout, "no hello in time");
      }, options.helloTimeoutMs),
      setInterval(() => {
        if (Date.now() - this.lastHeard > options.silenceMs) {
          this.ws.terminate();
          return;
        }
        if (this.phase === "open") this.send({ type: "ping" });
      }, options.heartbeatMs),
    );
  }

  send(message: ShareServerMessage) {
    this.sendText(JSON.stringify(message));
  }

  sendText(text: string) {
    if (this.ws.readyState !== this.ws.OPEN) return;
    if (this.ws.bufferedAmount > this.context.options.dropBytes) {
      this.close(ShareCloseCode.overloaded, "not reading");
      return;
    }
    this.ws.send(text);
  }

  end(reason: ShareEndReason) {
    if (!this.room) return;
    this.send({ type: "ended", share: this.room.id, reason });
    this.room = null;
    this.close(ShareCloseCode.ended, "the share ended");
  }

  bye(reconnectAfterMs: number) {
    this.send({ type: "bye", reconnectAfterMs });
    this.close(ShareCloseCode.restarting, "restarting");
  }

  private close(code: ShareCloseCode, reason: string) {
    if (this.phase === "closing") return;
    this.phase = "closing";
    this.ws.close(code, reason);
  }

  private serially(task: () => Promise<void>) {
    this.queue = this.queue.then(task).catch((error: unknown) => {
      this.log.error({ err: error }, "a share connection's work failed");
      this.close(ShareCloseCode.serverError, "server error");
    });
  }

  private onMessage(data: RawData, isBinary: boolean) {
    this.lastHeard = Date.now();
    const { options } = this.context;
    const size = Array.isArray(data)
      ? data.reduce((total, part) => total + part.length, 0)
      : data instanceof ArrayBuffer
        ? data.byteLength
        : data.length;
    const limit =
      this.role === "host"
        ? options.maxHostMessageBytes
        : options.maxViewerMessageBytes;
    if (size > limit) {
      this.close(ShareCloseCode.badMessage, "message too large");
      return;
    }
    const now = Date.now();
    if (now - this.messageWindow.started >= 1_000)
      this.messageWindow = { started: now, count: 0 };
    this.messageWindow.count += 1;
    const perSecond =
      this.role === "host"
        ? options.hostMessagesPerSecond
        : options.viewerMessagesPerSecond;
    if (this.messageWindow.count > perSecond) {
      this.close(ShareCloseCode.overLimit, "too many messages");
      return;
    }
    if (this.phase === "closing") return;

    let value: unknown;
    try {
      value = isBinary ? undefined : JSON.parse(data.toString());
    } catch {
      value = undefined;
    }
    const result = protocol.validate("ShareClientMessage", value);
    if (!result.ok) {
      this.close(ShareCloseCode.badMessage, "not a share message");
      return;
    }
    this.handle(result.value);
  }

  private handle(message: ShareClientMessage) {
    if (message.type === "hello") {
      if (this.phase !== "hello") {
        this.close(ShareCloseCode.badMessage, "one hello per connection");
        return;
      }
      this.phase = "proving";
      this.serially(() => this.prove(message));
      return;
    }
    if (this.phase !== "open") {
      this.close(ShareCloseCode.badMessage, "hello first");
      return;
    }
    if (message.type === "pong") return;
    if (this.role === "host") {
      switch (message.type) {
        case "open":
          this.serially(() => this.open(message));
          return;
        case "publish": {
          const room = this.hostRoom(message.share);
          if (room) room.publish(message.frame, this.context.options);
          return;
        }
        case "answer":
          this.hostRoom(message.share)
            ?.viewers.get(message.viewer)
            ?.send({ type: "answer", sealed: message.sealed });
          return;
        case "close":
          this.serially(() => this.closeShare(message.share));
          return;
      }
    } else {
      switch (message.type) {
        case "ask":
          this.room?.host?.send({
            type: "ask",
            share: this.room.id,
            viewer: this.id,
            sealed: message.sealed,
          });
          return;
        case "auth":
          this.serially(() => this.reauthenticate(message.token));
          return;
      }
    }
    this.close(ShareCloseCode.badMessage, `a ${this.role} does not send that`);
  }

  /** The room of a share this connection opened, or null. */
  private hostRoom(id: string): Room | null {
    const room = this.context.relay.rooms.get(id);
    return room && room.hostKey === this.key && room.host === this
      ? room
      : null;
  }

  /** Ends a share of this host's, whether or not it opened it again since it connected. */
  private async closeShare(id: string) {
    const row = await this.context.db
      .selectFrom("shares")
      .select(["host_key", "ended_at"])
      .where("id", "=", id)
      .executeTakeFirst();
    if (row?.host_key === this.key && !row.ended_at)
      await endShare(this.context, id, "stopped");
    this.send({ type: "ended", share: id, reason: "stopped" });
  }

  private async prove(hello: ShareHello) {
    try {
      if (Date.now() > this.challengeExpires)
        throw new HelloRefused("the challenge expired");
      if (hello.role === "host") await this.proveHost(hello);
      else await this.proveViewer(hello);
    } catch (error) {
      if (!(error instanceof HelloRefused || error instanceof ApiFailure))
        throw error;
      for (const key of failedShareHelloKeys(this.ip, hello.key))
        this.context.limiter.allow(
          key,
          this.context.options.failedHellosPerMinute,
        );
      this.log.info(
        { role: hello.role, why: error.message },
        "refused a share hello",
      );
      this.close(ShareCloseCode.unauthenticated, "not signed in");
    }
  }

  private async proveHost(hello: ShareHello) {
    const { db, limiter, options, relay, log } = this.context;
    if (!hello.key || !hello.signature)
      throw new HelloRefused("a host proves its key");
    if (
      limiter.exceeded(
        `share-failed-key:${hello.key}`,
        options.failedHellosPerMinute,
      )
    )
      throw new HelloRefused("too many failed hellos for this key");
    if (
      !signedBy(hello.key, shareMessage(this.nonce, hello.key), hello.signature)
    )
      throw new HelloRefused("the signature does not match");
    const device = await db
      .selectFrom("devices")
      .innerJoin("users", "users.id", "devices.user_id")
      .select(["devices.user_id", "devices.role", "users.deleted_at"])
      .where("devices.key", "=", hello.key)
      .executeTakeFirst();
    if (!device || device.role !== "host" || device.deleted_at)
      throw new HelloRefused("the key is not a host on any account");
    if (this.phase === "closing") return;

    this.key = hello.key;
    this.userId = device.user_id;
    this.role = "host";
    const older = relay.hosts.get(this.key);
    if (older instanceof ShareSocket)
      older.close(ShareCloseCode.replaced, "a newer connection for this host");
    relay.hosts.set(this.key, this);
    this.phase = "open";
    this.send({ type: "ready", heartbeatMs: options.heartbeatMs });
    log.info({ key: this.key.slice(0, 8) }, "a host opened its shares");
  }

  private async proveViewer(hello: ShareHello) {
    const { db, verifier, options, log } = this.context;
    if (!hello.token || !hello.share)
      throw new HelloRefused("a viewer sends its token and the share");
    const identity = await verifier.verify(hello.token);
    await checkStanding(db, identity);
    const row = await db
      .selectFrom("shares")
      .select(["id", "host_key", "expires_at", "ended_at", "ended_reason"])
      .where("id", "=", hello.share)
      .executeTakeFirst();
    if (this.phase === "closing") return;
    if (!row) {
      this.close(ShareCloseCode.notFound, "no such share");
      return;
    }
    this.userId = identity.userId;
    this.role = "viewer";
    if (row.ended_at || row.expires_at.getTime() <= Date.now()) {
      this.send({
        type: "ended",
        share: row.id,
        reason: row.ended_reason === "stopped" ? "stopped" : "expired",
      });
      this.close(ShareCloseCode.ended, "the share ended");
      return;
    }
    const room = roomFor(this.context, row);
    if (room.viewers.size >= options.viewersPerShare) {
      this.close(ShareCloseCode.overLimit, "too many viewers");
      return;
    }
    this.room = room;
    room.viewers.set(this.id, this);
    this.phase = "open";
    this.watchExpiry(identity.expiresAt);
    this.send({
      type: "ready",
      heartbeatMs: options.heartbeatMs,
      expiresAt: row.expires_at.toISOString(),
      hostOnline: room.host !== null,
      authExpiresAt: identity.expiresAt.toISOString(),
    });
    const frames = room.framesAfter(hello.since);
    for (let at = 0; at < frames.length; at += options.framesPerMessage)
      this.send({
        type: "frames",
        frames: frames.slice(at, at + options.framesPerMessage),
      });
    room.countViewers();
    log.info(
      { share: row.id, userId: identity.userId, viewers: room.viewers.size },
      "a viewer joined a share",
    );
  }

  private async open(message: ShareOpen) {
    const { db, relay, options } = this.context;
    const expiresAt = new Date(message.expiresAt);
    if (
      expiresAt.getTime() <= Date.now() ||
      expiresAt.getTime() > Date.now() + options.maxLifetimeMs
    ) {
      this.send({ type: "refused", share: message.share, reason: "expiry" });
      return;
    }
    const existing = await db
      .selectFrom("shares")
      .select(["id", "host_key", "expires_at", "ended_at"])
      .where("id", "=", message.share)
      .executeTakeFirst();
    if (existing) {
      if (existing.host_key !== this.key || existing.ended_at) {
        this.send({ type: "refused", share: message.share, reason: "exists" });
        return;
      }
    } else {
      const open = await db
        .selectFrom("shares")
        .select((eb) => eb.fn.countAll<string>().as("n"))
        .where("user_id", "=", this.userId)
        .where("ended_at", "is", null)
        .where("expires_at", ">", sql<Date>`now()`)
        .executeTakeFirstOrThrow();
      if (Number(open.n) >= options.openPerUser) {
        this.send({ type: "refused", share: message.share, reason: "limit" });
        return;
      }
      const inserted = await db
        .insertInto("shares")
        .values({
          id: message.share,
          user_id: this.userId,
          host_key: this.key,
          expires_at: expiresAt,
        })
        .onConflict((conflict) => conflict.column("id").doNothing())
        .returning("id")
        .executeTakeFirst();
      if (!inserted) {
        this.send({ type: "refused", share: message.share, reason: "exists" });
        return;
      }
      this.log.info(
        { share: message.share, userId: this.userId },
        "a share opened",
      );
    }
    if (this.phase !== "open") return;
    const room = roomFor(this.context, {
      id: message.share,
      host_key: this.key,
      expires_at: existing?.expires_at ?? expiresAt,
    });
    this.send({ type: "opened", share: message.share });
    if (relay.hosts.get(this.key) === this && room.host !== this)
      room.attachHost(this);
  }

  private watchExpiry(expiresAt: Date) {
    clearTimeout(this.authTimer);
    const delay =
      expiresAt.getTime() - Date.now() + this.context.options.authGraceMs;
    this.authTimer = setTimeout(
      () => this.close(ShareCloseCode.unauthenticated, "the sign-in expired"),
      Math.max(0, delay),
    );
  }

  private async reauthenticate(token: string) {
    const { verifier, db } = this.context;
    try {
      const identity = await verifier.verify(token);
      if (identity.userId !== this.userId)
        throw new HelloRefused("the token is for another account");
      await checkStanding(db, identity);
      this.watchExpiry(identity.expiresAt);
    } catch (error) {
      if (!(error instanceof HelloRefused || error instanceof ApiFailure))
        throw error;
      this.close(ShareCloseCode.unauthenticated, "not signed in");
    }
  }

  private onClose() {
    this.phase = "closing";
    for (const timer of this.timers) clearTimeout(timer);
    clearTimeout(this.authTimer);
    const { relay } = this.context;
    relay.sockets -= 1;
    if (this.role === "host") {
      if (relay.hosts.get(this.key) === this) relay.hosts.delete(this.key);
      for (const room of relay.rooms.values()) room.detachHost(this);
    } else if (this.room) {
      this.room.viewers.delete(this.id);
      this.room.countViewers();
      this.room = null;
    }
  }
}
