import { randomBytes } from "node:crypto";
import type {
  LiveDeviceMessage,
  LiveHello,
  LivePush,
  LiveRole,
  LiveServerMessage,
  RevokeReason,
} from "@sikemux/protocol";
import { LiveCloseCode, validator } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";
import type { RawData, WebSocket } from "ws";

import {
  checkStanding,
  isSession,
  isWeb,
  type Identity,
  type Verifier,
} from "../auth.ts";
import type { Tables } from "../db.ts";
import { removeDevice } from "../devices/removal.ts";
import { liveMessage, signedBy } from "../devices/signature.ts";
import { backlog } from "../events/backlog.ts";
import { latestEventId } from "../events/log.ts";
import { ApiFailure } from "../http.ts";
import type { RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";
import type { Pusher } from "../push/send.ts";
import type { Hub, Member } from "./hub.ts";
import type { LiveOptions } from "./options.ts";

export interface LiveContext {
  db: Kysely<Tables>;
  verifier: Verifier;
  hub: Hub;
  limiter: RateLimiter;
  log: Logger;
  options: LiveOptions;
  pusher: Pusher;
}

/** Who a hello proved the connection is. */
type Proven =
  | {
      kind: "device";
      role: "host" | "client";
      userId: string;
      key: string;
      cursor: number;
    }
  | { kind: "web"; userId: string; expiresAt: Date }
  | {
      kind: "farewell";
      role: "host" | "client";
      userId: string;
      key: string;
      cursor: number;
      reason: RevokeReason;
    };

class HelloRefused extends Error {}
class HelloLimited extends HelloRefused {}

const protocol = validator();

export function failedHelloKeys(ip: string, key?: string): string[] {
  return [`live-failed-ip:${ip}`, ...(key ? [`live-failed-key:${key}`] : [])];
}

/**
 * One device's connection: the challenge and hello, then events from its cursor, acknowledged
 * as it handles them, until it is closed, replaced or revoked.
 */
export class LiveSocket implements Member {
  userId = "";
  deviceKey: string | null = null;
  private role: LiveRole | null = null;
  private phase: "hello" | "proving" | "live" | "closing" = "hello";
  private readonly nonce = randomBytes(32).toString("hex");
  private readonly challengeExpires: number;
  private readonly opened = Date.now();
  private lastHeard = Date.now();
  private messageWindow = { started: 0, count: 0 };
  private sentThrough = 0;
  private ackedThrough = 0;
  private storedThrough = 0;
  private inFlight: number[] = [];
  private pushesInFlight = 0;
  private queue: Promise<void> = Promise.resolve();
  private wakeQueued = false;
  private readonly timers: NodeJS.Timeout[] = [];
  private authTimer: NodeJS.Timeout | undefined;
  private cursorTimer: NodeJS.Timeout | undefined;
  private lastCursorWrite = 0;
  private finished: Promise<void>;
  private finish!: () => void;
  private readonly log: Logger;

  private readonly ws: WebSocket;
  private readonly ip: string;
  private readonly context: LiveContext;

  constructor(ws: WebSocket, ip: string, context: LiveContext) {
    this.ws = ws;
    this.ip = ip;
    this.context = context;
    this.challengeExpires = Date.now() + context.options.challengeMs;
    this.log = context.log.child({ live: true });
    this.finished = new Promise((resolve) => {
      this.finish = resolve;
    });
  }

  start() {
    const { options, hub } = this.context;
    hub.opened(this);
    this.ws.on("message", (data, isBinary) => this.onMessage(data, isBinary));
    this.ws.on("close", (code) => void this.onClose(code));
    this.ws.on("error", (error) =>
      this.log.debug({ err: error }, "a live connection failed"),
    );
    this.send({
      type: "challenge",
      nonce: this.nonce,
      expiresAt: new Date(this.challengeExpires).toISOString(),
    });
    this.timers.push(
      setTimeout(() => {
        if (this.phase === "hello")
          this.close(LiveCloseCode.helloTimeout, "no hello in time");
      }, options.helloTimeoutMs),
      setInterval(() => {
        if (Date.now() - this.lastHeard > options.silenceMs) {
          this.ws.terminate();
          return;
        }
        if (this.phase === "live") this.send({ type: "ping" });
      }, options.heartbeatMs),
      setTimeout(() => {
        void this.bye(Math.round(Math.random() * 5_000));
      }, options.maxAgeMs),
    );
  }

  wake() {
    if (this.phase !== "live" || this.wakeQueued) return;
    this.wakeQueued = true;
    this.serially(async () => {
      this.wakeQueued = false;
      await this.deliver();
    });
  }

  replaced() {
    this.close(LiveCloseCode.replaced, "a newer connection for this device");
  }

  async bye(reconnectAfterMs: number) {
    if (this.phase === "closing") return this.finished;
    this.send({ type: "bye", reconnectAfterMs });
    this.close(LiveCloseCode.restarting, "restarting");
    return this.finished;
  }

  private send(message: LiveServerMessage) {
    if (this.ws.readyState === this.ws.OPEN)
      this.ws.send(JSON.stringify(message));
  }

  private close(code: LiveCloseCode, reason: string) {
    if (this.phase === "closing") return;
    this.phase = "closing";
    this.ws.close(code, reason);
  }

  private serially(task: () => Promise<void>) {
    this.queue = this.queue.then(task).catch((error: unknown) => {
      this.log.error({ err: error }, "a live connection's work failed");
      this.close(LiveCloseCode.serverError, "server error");
    });
  }

  private onMessage(data: RawData, isBinary: boolean) {
    this.lastHeard = Date.now();
    const { options } = this.context;
    const now = Date.now();
    if (now - this.messageWindow.started >= 1_000)
      this.messageWindow = { started: now, count: 0 };
    this.messageWindow.count += 1;
    if (this.messageWindow.count > options.messagesPerSecond) {
      this.close(LiveCloseCode.overLimit, "too many messages");
      return;
    }
    if (this.phase === "closing") return;

    let value: unknown;
    try {
      value = isBinary ? undefined : JSON.parse(data.toString());
    } catch {
      value = undefined;
    }
    const result = protocol.validate("LiveDeviceMessage", value);
    if (!result.ok) {
      this.close(LiveCloseCode.badMessage, "not a live message");
      return;
    }
    this.handle(result.value);
  }

  private handle(message: LiveDeviceMessage) {
    if (message.type === "hello") {
      if (this.phase !== "hello") {
        this.close(LiveCloseCode.badMessage, "one hello per connection");
        return;
      }
      this.phase = "proving";
      this.serially(() => this.prove(message));
      return;
    }
    if (this.phase !== "live") {
      this.close(LiveCloseCode.badMessage, "hello first");
      return;
    }
    switch (message.type) {
      case "pong":
        return;
      case "ack":
        this.onAck(message.id);
        return;
      case "auth":
        if (this.role !== "web") {
          this.close(LiveCloseCode.badMessage, "only the web app sends auth");
          return;
        }
        this.serially(() => this.reauthenticate(message.token));
        return;
      case "leave":
        if (this.role === "web") {
          this.close(LiveCloseCode.badMessage, "the web app is not a device");
          return;
        }
        this.serially(() => this.leave());
        return;
      case "push":
        if (this.role !== "host") {
          this.close(LiveCloseCode.badMessage, "only hosts push");
          return;
        }
        this.push(message);
        return;
    }
  }

  /** Pushes run beside the event stream, so a slow platform never holds up events. */
  private push(message: LivePush) {
    const { ref } = message;
    if (this.pushesInFlight >= this.context.options.pushesInFlight) {
      this.send({ type: "pushed", ref, result: "throttled" });
      return;
    }
    this.pushesInFlight += 1;
    this.context.pusher
      .push({ userId: this.userId, key: this.deviceKey ?? "" }, message)
      .catch((error: unknown) => {
        this.log.error({ err: error }, "a push failed");
        return "failed" as const;
      })
      .then((result) => {
        this.pushesInFlight -= 1;
        if (this.phase === "live") this.send({ type: "pushed", ref, result });
      });
  }

  private async prove(hello: LiveHello) {
    let proven: Proven;
    try {
      proven = await this.check(hello);
    } catch (error) {
      if (!(error instanceof HelloRefused || error instanceof ApiFailure))
        throw error;
      for (const key of failedHelloKeys(this.ip, hello.key))
        this.context.limiter.allow(
          key,
          this.context.options.failedHellosPerMinute,
        );
      this.log.info(
        { role: hello.role, key: hello.key?.slice(0, 8), why: error.message },
        "refused a live hello",
      );
      if (error instanceof HelloLimited)
        this.close(LiveCloseCode.overLimit, "too many failed hellos");
      else this.close(LiveCloseCode.unauthenticated, "not signed in");
      return;
    }
    if (this.phase === "closing") return;
    this.userId = proven.userId;
    this.role = proven.kind === "web" ? "web" : proven.role;

    if (proven.kind === "farewell") {
      this.deviceKey = proven.key;
      this.sentThrough = proven.cursor;
      await this.farewell(proven.reason);
      return;
    }

    const { hub, db, options } = this.context;
    if (proven.kind === "device") {
      this.deviceKey = proven.key;
      this.sentThrough = proven.cursor;
      this.ackedThrough = proven.cursor;
      this.storedThrough = proven.cursor;
    }
    if (!hub.admit(this)) {
      this.close(
        LiveCloseCode.overLimit,
        "too many connections for this account",
      );
      return;
    }
    this.phase = "live";

    const latest = await latestEventId(db, this.userId);
    if (proven.kind === "web") {
      this.sentThrough = latest;
      this.ackedThrough = latest;
      this.watchExpiry(proven.expiresAt);
      this.send({
        type: "ready",
        latest,
        heartbeatMs: options.heartbeatMs,
        authExpiresAt: proven.expiresAt.toISOString(),
      });
    } else {
      await db
        .updateTable("devices")
        .set({ last_seen_at: sql<Date>`now()` })
        .where("key", "=", proven.key)
        .execute();
      this.send({ type: "ready", latest, heartbeatMs: options.heartbeatMs });
      const user = await db
        .selectFrom("users")
        .select("events_pruned_through")
        .where("id", "=", this.userId)
        .executeTakeFirst();
      if (proven.cursor < Number(user?.events_pruned_through ?? 0))
        this.send({ type: "reset", latest });
    }
    this.log.info(
      {
        role: this.role,
        key: this.deviceKey?.slice(0, 8),
        userId: this.userId,
      },
      "a live connection opened",
    );
    await this.deliver();
  }

  /** Works out who the hello proves the connection is, or refuses it. */
  private async check(hello: LiveHello): Promise<Proven> {
    const { db, verifier, limiter, options } = this.context;
    if (Date.now() > this.challengeExpires)
      throw new HelloRefused("the challenge expired");

    if (hello.role === "web") {
      if (!hello.token) throw new HelloRefused("no token");
      const identity = await verifier.verify(hello.token);
      this.checkWebIdentity(identity);
      await checkStanding(db, identity);
      return {
        kind: "web",
        userId: identity.userId,
        expiresAt: identity.expiresAt,
      };
    }

    if (!hello.key || !hello.signature)
      throw new HelloRefused("a device proves its key");
    if (
      limiter.exceeded(
        `live-failed-key:${hello.key}`,
        options.failedHellosPerMinute,
      )
    )
      throw new HelloLimited("too many failed hellos for this key");
    if (
      !signedBy(hello.key, liveMessage(this.nonce, hello.key), hello.signature)
    )
      throw new HelloRefused("the signature does not match");

    let identity: Identity | undefined;
    if (hello.role === "client") {
      if (!hello.token) throw new HelloRefused("a client sends its session");
      identity = await verifier.verify(hello.token);
      if (!isSession(identity))
        throw new HelloRefused("a client signs in with a session token");
    }

    const device = await db
      .selectFrom("devices")
      .select(["user_id", "role", "acked_event_id"])
      .where("key", "=", hello.key)
      .executeTakeFirst();
    const owner = identity?.userId;
    if (device) {
      if (device.role !== hello.role)
        throw new HelloRefused("the key is registered with another role");
      if (owner !== undefined && device.user_id !== owner)
        throw new HelloRefused("the key belongs to another account");
      if (identity) await checkStanding(db, identity);
      return {
        kind: "device",
        role: hello.role,
        userId: device.user_id,
        key: hello.key,
        cursor: Number(device.acked_event_id),
      };
    }

    const gone = await db
      .selectFrom("removed_devices")
      .select(["user_id", "role", "acked_event_id", "reason"])
      .where("key", "=", hello.key)
      .executeTakeFirst();
    if (
      !gone ||
      gone.role !== hello.role ||
      (owner !== undefined && gone.user_id !== owner)
    )
      throw new HelloRefused("the key is not on any account");
    return {
      kind: "farewell",
      role: hello.role,
      userId: gone.user_id,
      key: hello.key,
      cursor: Number(gone.acked_event_id),
      reason: gone.reason as RevokeReason,
    };
  }

  private checkWebIdentity(identity: Identity) {
    if (!isWeb(identity))
      throw new HelloRefused("the token was not issued to the web app");
  }

  private watchExpiry(expiresAt: Date) {
    clearTimeout(this.authTimer);
    const delay =
      expiresAt.getTime() - Date.now() + this.context.options.authGraceMs;
    this.authTimer = setTimeout(
      () => this.close(LiveCloseCode.unauthenticated, "the sign-in expired"),
      Math.max(0, delay),
    );
  }

  private async reauthenticate(token: string) {
    const { verifier, db } = this.context;
    try {
      const identity = await verifier.verify(token);
      this.checkWebIdentity(identity);
      if (identity.userId !== this.userId)
        throw new HelloRefused("the token is for another account");
      await checkStanding(db, identity);
      this.watchExpiry(identity.expiresAt);
    } catch (error) {
      if (!(error instanceof HelloRefused || error instanceof ApiFailure))
        throw error;
      this.close(LiveCloseCode.unauthenticated, "not signed in");
    }
  }

  /** Whether the connection may still read the account, or why it may not. */
  private async standing(): Promise<RevokeReason | null> {
    const { db } = this.context;
    if (this.role === "web") {
      const user = await db
        .selectFrom("users")
        .select("deleted_at")
        .where("id", "=", this.userId)
        .executeTakeFirst();
      return user?.deleted_at ? "account_deleted" : null;
    }
    const key = this.deviceKey ?? "";
    const device = await db
      .selectFrom("devices")
      .select("user_id")
      .where("key", "=", key)
      .executeTakeFirst();
    if (device?.user_id === this.userId) return null;
    const gone = await db
      .selectFrom("removed_devices")
      .select("reason")
      .where("key", "=", key)
      .executeTakeFirst();
    return (gone?.reason as RevokeReason | undefined) ?? "removed";
  }

  private async deliver() {
    if (this.phase !== "live") return;
    const reason = await this.standing();
    if (reason) {
      await this.farewell(reason);
      return;
    }
    const { options, db } = this.context;
    while (
      this.phase === "live" &&
      this.inFlight.length < options.framesAhead
    ) {
      if (this.ws.bufferedAmount > options.dropBytes) {
        this.close(LiveCloseCode.overloaded, "not reading");
        return;
      }
      if (this.ws.bufferedAmount > options.pauseBytes) {
        setTimeout(() => this.wake(), 100);
        return;
      }
      const events = await backlog(db, {
        userId: this.userId,
        role: this.role ?? "web",
        key: this.deviceKey,
        after: this.sentThrough,
        limit: options.eventsPerFrame,
      });
      const last = events.at(-1);
      if (!last) return;
      this.send({ type: "events", events });
      this.sentThrough = last.id;
      this.inFlight.push(last.id);
    }
  }

  /** Sends everything the device has not heard, then why it is gone, and closes. */
  private async farewell(reason: RevokeReason) {
    if (this.phase === "closing") return;
    const { db, options } = this.context;
    for (;;) {
      const events = await backlog(db, {
        userId: this.userId,
        role: this.role ?? "web",
        key: this.deviceKey,
        after: this.sentThrough,
        limit: options.eventsPerFrame,
      });
      const last = events.at(-1);
      if (!last) break;
      this.send({ type: "events", events });
      this.sentThrough = last.id;
    }
    this.send({ type: "revoked", reason });
    this.log.info(
      { role: this.role, key: this.deviceKey?.slice(0, 8), reason },
      "told a live connection it was revoked",
    );
    this.close(LiveCloseCode.revoked, "revoked");
  }

  private onAck(id: number) {
    const acked = Math.min(id, this.sentThrough);
    if (acked <= this.ackedThrough) return;
    this.ackedThrough = acked;
    this.inFlight = this.inFlight.filter((last) => last > acked);
    if (this.role !== "web") this.scheduleCursorWrite();
    this.wake();
  }

  private scheduleCursorWrite() {
    if (this.cursorTimer) return;
    const wait = Math.max(
      0,
      this.lastCursorWrite + this.context.options.cursorFlushMs - Date.now(),
    );
    this.cursorTimer = setTimeout(() => {
      this.cursorTimer = undefined;
      this.serially(() => this.writeCursor());
    }, wait);
  }

  private async writeCursor() {
    if (!this.deviceKey || this.ackedThrough <= this.storedThrough) return;
    const through = this.ackedThrough;
    this.lastCursorWrite = Date.now();
    await this.context.db
      .updateTable("devices")
      .set({
        acked_event_id: sql<string>`greatest(acked_event_id, ${through})`,
      })
      .where("key", "=", this.deviceKey)
      .where("user_id", "=", this.userId)
      .execute();
    this.storedThrough = through;
  }

  private async leave() {
    const key = this.deviceKey;
    if (!key) return;
    await this.writeCursor();
    await this.context.db.transaction().execute((trx) =>
      removeDevice(trx, {
        userId: this.userId,
        key,
        reason: "signed_out",
        actor: `device:${key}`,
      }),
    );
    this.log.info(
      { key: key.slice(0, 8), role: this.role },
      "a device left its account",
    );
    await this.farewell("signed_out");
  }

  private async onClose(code: number) {
    this.phase = "closing";
    for (const timer of this.timers) clearTimeout(timer);
    clearTimeout(this.authTimer);
    clearTimeout(this.cursorTimer);
    const { hub, db } = this.context;
    hub.closed(this);
    try {
      await this.queue;
      if (this.role === "host" || this.role === "client") {
        await this.writeCursor();
        await db
          .updateTable("devices")
          .set({ last_seen_at: sql<Date>`now()` })
          .where("key", "=", this.deviceKey ?? "")
          .where("user_id", "=", this.userId)
          .execute();
      }
    } catch (error) {
      this.log.warn(
        { err: error },
        "could not record a closed live connection",
      );
    }
    this.log.info(
      {
        role: this.role,
        key: this.deviceKey?.slice(0, 8),
        code,
        ms: Date.now() - this.opened,
      },
      "a live connection closed",
    );
    this.finish();
  }
}
