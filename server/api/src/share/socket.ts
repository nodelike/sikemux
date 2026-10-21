import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import type {
  ShareClientMessage,
  ShareEndReason,
  ShareHello,
  ShareOpen,
  ShareServerMessage,
} from "@sikemux/protocol";
import { ShareCloseCode } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";
import type { WebSocket } from "ws";

import { checkStanding, type Identity } from "../auth.ts";
import type { Tables } from "../db.ts";
import {
  shareMessage,
  shareViewerMessage,
  signedBy,
} from "../devices/signature.ts";
import {
  ChallengedSocket,
  HelloRefused,
  type SocketContext,
  type SocketKind,
} from "../sockets/socket.ts";
import type { ShareOptions } from "./options.ts";
import { Room, type HostLink, type Relay, type ViewerLink } from "./relay.ts";

export interface ShareContext extends SocketContext {
  relay: Relay;
  options: ShareOptions;
}

export const SHARE_SOCKET: SocketKind = {
  name: "share",
  incoming: "ShareClientMessage",
  codes: ShareCloseCode,
};

/** A viewer signed in with a token carries it, so its expiry can close the connection. */
type Who =
  | { role: "host" }
  | {
      role: "viewer";
      room: Room;
      identity: Identity | null;
      since: ShareHello["since"];
    };

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

/** The account each of `keys` is a host on, for those still on an account that stands. */
async function hostAccounts(
  db: Kysely<Tables>,
  keys: string[],
): Promise<Map<string, string>> {
  if (!keys.length) return new Map();
  const rows = await db
    .selectFrom("devices")
    .innerJoin("users", "users.id", "devices.user_id")
    .select(["devices.key", "devices.user_id"])
    .where("devices.key", "in", keys)
    .where("devices.role", "=", "host")
    .where("users.deleted_at", "is", null)
    .execute();
  return new Map(rows.map((row) => [row.key, row.user_id]));
}

/**
 * After an account changed, or after changes may have been missed when `userId` is undefined:
 * ends the rooms whose shares ended in the database, as removing their host does, and closes
 * the connections of hosts no longer on their account.
 */
export async function recheckShares(
  context: Pick<ShareContext, "db" | "relay">,
  userId?: string,
) {
  const { db, relay } = context;
  const ours = (owner: string) => userId === undefined || owner === userId;
  const rooms = [...relay.rooms.values()].filter((room) => ours(room.userId));
  const ended = rooms.length
    ? await db
        .selectFrom("shares")
        .select(["id", "ended_reason"])
        .where(
          "id",
          "in",
          rooms.map((room) => room.id),
        )
        .where("ended_at", "is not", null)
        .execute()
    : [];
  for (const row of ended) {
    const room = relay.rooms.get(row.id);
    relay.rooms.delete(row.id);
    room?.end(row.ended_reason === "expired" ? "expired" : "stopped");
  }
  const hosts = [
    ...relay.hosts.values(),
    ...[...relay.rooms.values()].flatMap((room) => [...room.viewers.values()]),
  ].filter((link) => link.key && ours(link.userId));
  const standing = await hostAccounts(
    db,
    hosts.map((host) => host.key),
  );
  for (const host of hosts)
    if (standing.get(host.key) !== host.userId) host.removed();
}

/**
 * Whether `proof` is what the share's link holds, by the check its host gave. Hosts on 0.5.3 and
 * earlier give none, and their shares are open to anyone signed in who has the link.
 */
function provesLink(proof: string | undefined, check: string | null): boolean {
  if (!check) return true;
  if (!proof) return false;
  const given = createHash("sha256")
    .update(Buffer.from(proof, "base64url"))
    .digest();
  return timingSafeEqual(given, Buffer.from(check, "hex"));
}

/** The room for an open share, made from its row when this process has not held it yet. */
function roomFor(
  context: ShareContext,
  row: { id: string; user_id: string; host_key: string; expires_at: Date },
): Room {
  const { relay } = context;
  let room = relay.rooms.get(row.id);
  if (!room) {
    room = new Room(
      row.id,
      row.user_id,
      row.host_key,
      row.expires_at,
      (expired) => {
        endShare(context, expired.id, "expired").catch((error: unknown) =>
          context.log.error({ err: error }, "ending an expired share failed"),
        );
      },
    );
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
export class ShareSocket
  extends ChallengedSocket<ShareClientMessage, ShareHello, Who>
  implements HostLink, ViewerLink
{
  readonly id = randomBytes(9).toString("base64url");
  key = "";
  userId = "";
  private role: ShareHello["role"] = "viewer";
  private room: Room | null = null;

  private readonly context: ShareContext;

  constructor(ws: WebSocket, ip: string, context: ShareContext) {
    super(ws, ip, context, SHARE_SOCKET);
    this.context = context;
  }

  override start() {
    this.context.relay.sockets += 1;
    super.start();
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

  removed() {
    this.close(ShareCloseCode.unauthenticated, "no longer signed in");
  }

  bye(reconnectAfterMs: number) {
    this.send({ type: "bye", reconnectAfterMs });
    this.close(ShareCloseCode.restarting, "restarting");
  }

  protected limits() {
    const { options } = this.context;
    return this.role === "host"
      ? {
          perSecond: options.hostMessagesPerSecond,
          bytes: options.maxHostMessageBytes,
        }
      : {
          perSecond: options.viewerMessagesPerSecond,
          bytes: options.maxViewerMessageBytes,
        };
  }

  protected handle(message: Exclude<ShareClientMessage, ShareHello>, who: Who) {
    if (who.role === "host") {
      switch (message.type) {
        case "open":
          this.serially(() => this.open(message));
          return;
        case "publish":
          this.hostRoom(message.share)?.publish(
            message.frame,
            this.context.options,
          );
          return;
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
          this.reauthenticate(message.token, this.userId, () => !this.key);
          return;
      }
    }
    this.refuse(`a ${who.role} does not send that`);
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

  protected prove(hello: ShareHello): Promise<Who | undefined> {
    return hello.role === "host"
      ? this.proveHost(hello)
      : this.proveViewer(hello);
  }

  protected opened(who: Who) {
    const { options, log } = this.context;
    if (who.role === "host") {
      this.send({ type: "ready", heartbeatMs: options.heartbeatMs });
      log.info({ key: this.key.slice(0, 8) }, "a host opened its shares");
      return;
    }
    const { room, identity, since } = who;
    if (identity) this.watchExpiry(identity.expiresAt);
    this.send({
      type: "ready",
      heartbeatMs: options.heartbeatMs,
      expiresAt: room.expiresAt.toISOString(),
      hostOnline: room.host !== null,
      ...(identity ? { authExpiresAt: identity.expiresAt.toISOString() } : {}),
    });
    const frames = room.framesAfter(since);
    for (let at = 0; at < frames.length; at += options.framesPerMessage)
      this.send({
        type: "frames",
        frames: frames.slice(at, at + options.framesPerMessage),
      });
    room.countViewers();
    log.info(
      { share: room.id, userId: this.userId, viewers: room.viewers.size },
      "a viewer joined a share",
    );
  }

  private async proveHost(hello: ShareHello): Promise<Who | undefined> {
    const { db, relay } = this.context;
    if (!hello.key || !hello.signature)
      throw new HelloRefused("a host proves its key");
    if (
      !signedBy(hello.key, shareMessage(this.nonce, hello.key), hello.signature)
    )
      throw new HelloRefused("the signature does not match");
    const account = (await hostAccounts(db, [hello.key])).get(hello.key);
    if (!account)
      throw new HelloRefused("the key is not a host on any account");
    if (this.phase === "closing") return undefined;

    this.key = hello.key;
    this.userId = account;
    this.role = "host";
    const older = relay.hosts.get(this.key);
    if (older instanceof ShareSocket)
      older.close(ShareCloseCode.replaced, "a newer connection for this host");
    relay.hosts.set(this.key, this);
    return { role: "host" };
  }

  /** Who a viewer is: a phone or the web app by its token, or a Mac by its host's key. */
  private async viewer(
    hello: ShareHello,
  ): Promise<{ userId: string; key: string; identity: Identity | null }> {
    const { db, verifier } = this.context;
    if (hello.token) {
      const identity = await verifier.verify(hello.token);
      await checkStanding(db, identity);
      return { userId: identity.userId, key: "", identity };
    }
    if (!hello.key || !hello.signature)
      throw new HelloRefused("a viewer sends its token or its host's key");
    if (
      !signedBy(
        hello.key,
        shareViewerMessage(this.nonce, hello.key),
        hello.signature,
      )
    )
      throw new HelloRefused("the signature does not match");
    const account = (await hostAccounts(db, [hello.key])).get(hello.key);
    if (!account)
      throw new HelloRefused("the key is not a host on any account");
    return { userId: account, key: hello.key, identity: null };
  }

  private async proveViewer(hello: ShareHello): Promise<Who | undefined> {
    const { db, options } = this.context;
    if (!hello.share) throw new HelloRefused("a viewer names the share");
    const viewer = await this.viewer(hello);
    const row = await db
      .selectFrom("shares")
      .select([
        "id",
        "user_id",
        "host_key",
        "expires_at",
        "ended_at",
        "ended_reason",
        "viewer_check",
      ])
      .where("id", "=", hello.share)
      .executeTakeFirst();
    if (this.phase === "closing") return undefined;
    if (!row || !provesLink(hello.proof, row.viewer_check)) {
      this.close(ShareCloseCode.notFound, "no such share");
      return undefined;
    }
    this.key = viewer.key;
    this.userId = viewer.userId;
    if (row.ended_at || row.expires_at.getTime() <= Date.now()) {
      this.send({
        type: "ended",
        share: row.id,
        reason: row.ended_reason === "stopped" ? "stopped" : "expired",
      });
      this.close(ShareCloseCode.ended, "the share ended");
      return undefined;
    }
    const room = roomFor(this.context, row);
    if (room.viewers.size >= options.viewersPerShare) {
      this.close(ShareCloseCode.overLimit, "too many viewers");
      return undefined;
    }
    this.room = room;
    room.viewers.set(this.id, this);
    return {
      role: "viewer",
      room,
      identity: viewer.identity,
      since: hello.since,
    };
  }

  private async open(message: ShareOpen) {
    const { db, relay, options } = this.context;
    if ((await hostAccounts(db, [this.key])).get(this.key) !== this.userId) {
      this.removed();
      return;
    }
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
      .select(["id", "host_key", "expires_at", "ended_at", "viewer_check"])
      .where("id", "=", message.share)
      .executeTakeFirst();
    if (existing) {
      if (
        existing.host_key !== this.key ||
        existing.ended_at ||
        existing.viewer_check !== (message.viewers ?? null)
      ) {
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
          viewer_check: message.viewers ?? null,
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
      user_id: this.userId,
      host_key: this.key,
      expires_at: existing?.expires_at ?? expiresAt,
    });
    this.send({ type: "opened", share: message.share });
    if (relay.hosts.get(this.key) === this && room.host !== this)
      room.attachHost(this);
  }

  protected closed() {
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
