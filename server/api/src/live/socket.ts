import type {
  LiveDeviceMessage,
  LiveHello,
  LivePush,
  LiveServerMessage,
  RevokeReason,
} from "@sikemux/protocol";
import { LiveCloseCode } from "@sikemux/protocol";
import { sql } from "kysely";
import type { WebSocket } from "ws";

import { checkStanding, isSession, isWeb, type Identity } from "../auth.ts";
import { removeDevice } from "../devices/removal.ts";
import { liveMessage, signedBy } from "../devices/signature.ts";
import { backlog } from "../events/backlog.ts";
import { latestEventId } from "../events/log.ts";
import type { Pusher } from "../push/send.ts";
import {
  ChallengedSocket,
  HelloRefused,
  type SocketContext,
  type SocketKind,
} from "../sockets/socket.ts";
import type { Hub, Member } from "./hub.ts";
import type { LiveOptions } from "./options.ts";

export interface LiveContext extends SocketContext {
  hub: Hub;
  options: LiveOptions;
  pusher: Pusher;
}

export const LIVE_SOCKET: SocketKind = {
  name: "live",
  incoming: "LiveDeviceMessage",
  codes: LiveCloseCode,
};

/** Who a hello proved the connection is. */
type Who =
  | { role: "web"; userId: string; expiresAt: Date }
  | {
      role: "host" | "client";
      userId: string;
      key: string;
      cursor: number;
      /** Why the device is no longer on the account, when it is not. */
      gone?: RevokeReason;
    };

type Device = Extract<Who, { key: string }>;

const keyOf = (who: Who) => (who.role === "web" ? null : who.key);

/**
 * One device's connection: the challenge and hello, then events from its cursor, acknowledged
 * as it handles them, until it is closed, replaced or revoked.
 */
export class LiveSocket extends ChallengedSocket<
  LiveDeviceMessage,
  LiveHello,
  Who
> {
  private readonly opening = Date.now();
  private sentThrough = 0;
  private ackedThrough = 0;
  private storedThrough = 0;
  private inFlight: number[] = [];
  private pushesInFlight = 0;
  private wakeQueued = false;
  private member: Member | undefined;
  private cursorTimer: NodeJS.Timeout | undefined;
  private lastCursorWrite = 0;
  private finished: Promise<void>;
  private finish!: () => void;

  private readonly context: LiveContext;

  constructor(ws: WebSocket, ip: string, context: LiveContext) {
    super(ws, ip, context, LIVE_SOCKET);
    this.context = context;
    this.finished = new Promise((resolve) => {
      this.finish = resolve;
    });
  }

  override start() {
    this.context.hub.opened(this);
    super.start([
      setTimeout(() => {
        void this.bye(Math.round(Math.random() * 5_000));
      }, this.context.options.maxAgeMs),
    ]);
  }

  async bye(reconnectAfterMs: number) {
    if (this.phase === "closing") return this.finished;
    this.send({ type: "bye", reconnectAfterMs });
    this.close(LiveCloseCode.restarting, "restarting");
    return this.finished;
  }

  protected limits() {
    const { messagesPerSecond, maxPayloadBytes } = this.context.options;
    return { perSecond: messagesPerSecond, bytes: maxPayloadBytes };
  }

  private send(message: LiveServerMessage) {
    this.write(message);
  }

  wake(who: Who) {
    if (this.phase !== "open" || this.wakeQueued) return;
    this.wakeQueued = true;
    this.serially(async () => {
      this.wakeQueued = false;
      await this.deliver(who);
    });
  }

  protected handle(message: Exclude<LiveDeviceMessage, LiveHello>, who: Who) {
    switch (message.type) {
      case "pong":
        return;
      case "ack":
        this.onAck(message.id, who);
        return;
      case "auth":
        if (who.role !== "web")
          return this.refuse("only the web app sends auth");
        this.reauthenticate(message.token, who.userId, isWeb);
        return;
      case "leave":
        if (who.role === "web")
          return this.refuse("the web app is not a device");
        this.serially(() => this.leave(who));
        return;
      case "push":
        if (who.role !== "host") return this.refuse("only hosts push");
        this.push(message, who);
        return;
    }
  }

  /** Pushes run beside the event stream, so a slow platform never holds up events. */
  private push(message: LivePush, host: Device) {
    const { ref } = message;
    if (this.pushesInFlight >= this.context.options.pushesInFlight) {
      this.send({ type: "pushed", ref, result: "throttled" });
      return;
    }
    this.pushesInFlight += 1;
    this.context.pusher
      .push({ userId: host.userId, key: host.key }, message)
      .catch((error: unknown) => {
        this.log.error({ err: error }, "a push failed");
        return "failed" as const;
      })
      .then((result) => {
        this.pushesInFlight -= 1;
        if (this.phase === "open") this.send({ type: "pushed", ref, result });
      });
  }

  protected async prove(hello: LiveHello): Promise<Who | undefined> {
    const who = await this.check(hello);
    if (who.role !== "web") {
      this.sentThrough = who.cursor;
      this.ackedThrough = who.cursor;
      this.storedThrough = who.cursor;
      if (who.gone) {
        await this.farewell(who, who.gone);
        return undefined;
      }
    }
    const member: Member = {
      userId: who.userId,
      deviceKey: keyOf(who),
      wake: () => this.wake(who),
      replaced: () =>
        this.close(
          LiveCloseCode.replaced,
          "a newer connection for this device",
        ),
    };
    if (!this.context.hub.admit(member)) {
      this.close(
        LiveCloseCode.overLimit,
        "too many connections for this account",
      );
      return undefined;
    }
    this.member = member;
    return who;
  }

  protected async opened(who: Who) {
    const { db, options } = this.context;
    const latest = await latestEventId(db, who.userId);
    if (who.role === "web") {
      this.sentThrough = latest;
      this.ackedThrough = latest;
      this.watchExpiry(who.expiresAt);
      this.send({
        type: "ready",
        latest,
        heartbeatMs: options.heartbeatMs,
        authExpiresAt: who.expiresAt.toISOString(),
      });
    } else {
      await db
        .updateTable("devices")
        .set({ last_seen_at: sql<Date>`now()` })
        .where("key", "=", who.key)
        .execute();
      this.send({ type: "ready", latest, heartbeatMs: options.heartbeatMs });
      const user = await db
        .selectFrom("users")
        .select("events_pruned_through")
        .where("id", "=", who.userId)
        .executeTakeFirstOrThrow();
      if (who.cursor < Number(user.events_pruned_through))
        this.send({ type: "reset", latest });
    }
    this.log.info(
      { role: who.role, key: keyOf(who)?.slice(0, 8), userId: who.userId },
      "a live connection opened",
    );
    await this.deliver(who);
  }

  /** Works out who the hello proves the connection is, or refuses it. */
  private async check(hello: LiveHello): Promise<Who> {
    const { db, verifier } = this.context;
    if (hello.role === "web") {
      if (!hello.token) throw new HelloRefused("no token");
      const identity = await verifier.verify(hello.token);
      if (!isWeb(identity))
        throw new HelloRefused("the token was not issued to the web app");
      await checkStanding(db, identity);
      return {
        role: "web",
        userId: identity.userId,
        expiresAt: identity.expiresAt,
      };
    }

    if (!hello.key || !hello.signature)
      throw new HelloRefused("a device proves its key");
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
      role: hello.role,
      userId: gone.user_id,
      key: hello.key,
      cursor: Number(gone.acked_event_id),
      gone: gone.reason as RevokeReason,
    };
  }

  /** Whether the connection may still read the account, or why it may not. */
  private async standing(who: Who): Promise<RevokeReason | null> {
    const { db } = this.context;
    if (who.role === "web") {
      const user = await db
        .selectFrom("users")
        .select("deleted_at")
        .where("id", "=", who.userId)
        .executeTakeFirst();
      return user?.deleted_at ? "account_deleted" : null;
    }
    const device = await db
      .selectFrom("devices")
      .select("user_id")
      .where("key", "=", who.key)
      .executeTakeFirst();
    if (device?.user_id === who.userId) return null;
    const gone = await db
      .selectFrom("removed_devices")
      .select("reason")
      .where("key", "=", who.key)
      .executeTakeFirst();
    return (gone?.reason as RevokeReason | undefined) ?? "removed";
  }

  /** The next frame of events after what was sent, or nothing when the device has them all. */
  private async nextFrame(who: Who): Promise<boolean> {
    const events = await backlog(this.context.db, {
      userId: who.userId,
      role: who.role,
      key: keyOf(who),
      after: this.sentThrough,
      limit: this.context.options.eventsPerFrame,
    });
    const last = events.at(-1);
    if (!last) return false;
    this.send({ type: "events", events });
    this.sentThrough = last.id;
    return true;
  }

  private async deliver(who: Who) {
    if (this.phase !== "open") return;
    const reason = await this.standing(who);
    if (reason) {
      await this.farewell(who, reason);
      return;
    }
    const { options } = this.context;
    while (
      this.phase === "open" &&
      this.inFlight.length < options.framesAhead
    ) {
      if (this.ws.bufferedAmount > options.dropBytes) {
        this.close(LiveCloseCode.overloaded, "not reading");
        return;
      }
      if (this.ws.bufferedAmount > options.pauseBytes) {
        setTimeout(() => this.wake(who), 100);
        return;
      }
      if (!(await this.nextFrame(who))) return;
      this.inFlight.push(this.sentThrough);
    }
  }

  /** Sends everything the device has not heard, then why it is gone, and closes. */
  private async farewell(who: Who, reason: RevokeReason) {
    if (this.phase === "closing") return;
    while (await this.nextFrame(who));
    this.send({ type: "revoked", reason });
    this.log.info(
      { role: who.role, key: keyOf(who)?.slice(0, 8), reason },
      "told a live connection it was revoked",
    );
    this.close(LiveCloseCode.revoked, "revoked");
  }

  private onAck(id: number, who: Who) {
    const acked = Math.min(id, this.sentThrough);
    if (acked <= this.ackedThrough) return;
    this.ackedThrough = acked;
    this.inFlight = this.inFlight.filter((last) => last > acked);
    if (who.role !== "web") this.scheduleCursorWrite(who);
    this.wake(who);
  }

  private scheduleCursorWrite(device: Device) {
    if (this.cursorTimer) return;
    const wait = Math.max(
      0,
      this.lastCursorWrite + this.context.options.cursorFlushMs - Date.now(),
    );
    this.cursorTimer = setTimeout(() => {
      this.cursorTimer = undefined;
      this.serially(() => this.writeCursor(device));
    }, wait);
  }

  private async writeCursor(device: Device) {
    if (this.ackedThrough <= this.storedThrough) return;
    const through = this.ackedThrough;
    this.lastCursorWrite = Date.now();
    await this.context.db
      .updateTable("devices")
      .set({
        acked_event_id: sql<string>`greatest(acked_event_id, ${through})`,
      })
      .where("key", "=", device.key)
      .where("user_id", "=", device.userId)
      .execute();
    this.storedThrough = through;
  }

  private async leave(device: Device) {
    await this.writeCursor(device);
    await this.context.db.transaction().execute((trx) =>
      removeDevice(trx, {
        userId: device.userId,
        key: device.key,
        reason: "signed_out",
        actor: `device:${device.key}`,
      }),
    );
    this.log.info(
      { key: device.key.slice(0, 8), role: device.role },
      "a device left its account",
    );
    await this.farewell(device, "signed_out");
  }

  protected async closed(code: number, who: Who | undefined) {
    clearTimeout(this.cursorTimer);
    const { hub, db } = this.context;
    hub.closed(this);
    if (this.member) hub.remove(this.member);
    try {
      await this.settled();
      if (who && who.role !== "web") {
        await this.writeCursor(who);
        await db
          .updateTable("devices")
          .set({ last_seen_at: sql<Date>`now()` })
          .where("key", "=", who.key)
          .where("user_id", "=", who.userId)
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
        role: who?.role,
        key: who && keyOf(who)?.slice(0, 8),
        code,
        ms: Date.now() - this.opening,
      },
      "a live connection closed",
    );
    this.finish();
  }
}
