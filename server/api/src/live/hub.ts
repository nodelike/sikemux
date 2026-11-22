import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import type { LiveOptions } from "./options.ts";
import { byeAll } from "../sockets/socket.ts";

export interface Connection {
  bye(reconnectAfterMs: number): Promise<void>;
}

export interface Member {
  readonly userId: string;
  /** The device's key; the web app has none. */
  readonly deviceKey: string | null;
  wake(): void;
  replaced(): void;
}

/** The connections that have said who they are, by account and by device. */
export class Hub {
  private readonly byUser = new Map<string, Set<Member>>();
  private readonly byDevice = new Map<string, Member>();
  private readonly pending = new Set<string>();
  private readonly lastSwept = new Map<string, number>();
  private readonly connections = new Set<Connection>();
  private flushTimer: NodeJS.Timeout | undefined;

  private readonly options: LiveOptions;

  constructor(options: LiveOptions) {
    this.options = options;
  }

  /** Every open connection, including those still proving who they are. */
  get open(): number {
    return this.connections.size;
  }

  opened(connection: Connection) {
    this.connections.add(connection);
  }

  closed(connection: Connection) {
    this.connections.delete(connection);
  }

  /** Adds a connection, replacing an older one for the same device. False when its account has too many. */
  admit(member: Member): boolean {
    const older = member.deviceKey
      ? this.byDevice.get(member.deviceKey)
      : undefined;
    if (older) {
      this.remove(older);
      older.replaced();
    }
    const sockets = this.byUser.get(member.userId) ?? new Set();
    if (sockets.size >= this.options.socketsPerUser) return false;
    sockets.add(member);
    this.byUser.set(member.userId, sockets);
    if (member.deviceKey) this.byDevice.set(member.deviceKey, member);
    return true;
  }

  /** Whether the device is connected now. */
  online(deviceKey: string): boolean {
    return this.byDevice.has(deviceKey);
  }

  remove(member: Member) {
    const sockets = this.byUser.get(member.userId);
    sockets?.delete(member);
    if (sockets?.size === 0) {
      this.byUser.delete(member.userId);
      this.lastSwept.delete(member.userId);
    }
    if (member.deviceKey && this.byDevice.get(member.deviceKey) === member)
      this.byDevice.delete(member.deviceKey);
  }

  /** Wakes the account's connections, once for every notification within the coalescing window. */
  wake(userId: string) {
    if (!this.byUser.has(userId)) return;
    this.pending.add(userId);
    this.flushTimer ??= setTimeout(() => {
      this.flushTimer = undefined;
      const users = [...this.pending];
      this.pending.clear();
      for (const user of users)
        for (const member of this.byUser.get(user) ?? []) member.wake();
    }, this.options.coalesceMs);
  }

  wakeAll() {
    for (const sockets of this.byUser.values())
      for (const member of sockets) member.wake();
  }

  /** Wakes every account whose newest event moved since the last sweep, catching lost notifications. */
  async sweep(db: Kysely<Tables>, log: Logger) {
    const users = [...this.byUser.keys()];
    if (users.length === 0) return;
    const rows = await db
      .selectFrom("events")
      .select(["user_id", (eb) => eb.fn.max("id").as("latest")])
      .where("user_id", "in", users)
      .groupBy("user_id")
      .execute();
    let woken = 0;
    for (const row of rows) {
      const latest = Number(row.latest);
      if (latest > (this.lastSwept.get(row.user_id) ?? 0)) {
        this.lastSwept.set(row.user_id, latest);
        for (const member of this.byUser.get(row.user_id) ?? []) member.wake();
        woken += 1;
      }
    }
    log.debug({ users: users.length, woken }, "swept live connections");
  }

  closeAll(spreadMs: number) {
    return byeAll(this.connections, spreadMs);
  }
}

/** How long a host may be online without being connected, so a refusal waits for it. */
export interface PresenceWindows {
  /** After the API starts: a core comes back within five minutes of losing its connection. */
  reconnectMs: number;
  /** After a host was last seen, between a crash and its next connection. */
  seenMs: number;
}

const WINDOWS: PresenceWindows = {
  reconnectMs: 7 * 60_000,
  seenMs: 2 * 60_000,
};

/** Which devices are connected now, as the live server's hub knows once it is attached. */
export class Presence {
  private hub: Pick<Hub, "online"> | undefined;
  private attachedAt = 0;
  private readonly windows: PresenceWindows;

  constructor(windows = WINDOWS) {
    this.windows = windows;
  }

  attach(hub: Pick<Hub, "online">, at = Date.now()) {
    this.hub = hub;
    this.attachedAt = at;
  }

  online(deviceKey: string): boolean {
    return this.hub?.online(deviceKey) ?? false;
  }

  /**
   * Whether the host may be connected: connected now, seen moments ago, or the API started too
   * recently to tell, since a host connected before a restart has yet to come back.
   */
  mayBeOnline(deviceKey: string, lastSeenAt: Date | null, now = Date.now()) {
    if (!this.hub || now - this.attachedAt < this.windows.reconnectMs)
      return true;
    if (this.hub.online(deviceKey)) return true;
    return (
      lastSeenAt !== null && now - lastSeenAt.getTime() < this.windows.seenMs
    );
  }
}
