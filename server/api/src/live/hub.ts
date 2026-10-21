import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import type { LiveOptions } from "./options.ts";

/** What the hub needs of every open connection. */
export interface Connection {
  bye(reconnectAfterMs: number): Promise<void>;
}

/** What the hub needs of a connection that has said who it is. */
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

  /** Says bye to every connection, each told to wait a different time so they come back spread out. */
  async closeAll(spreadMs: number) {
    const connections = [...this.connections];
    const least = Math.min(1_000, spreadMs);
    await Promise.all(
      connections.map((connection) =>
        connection.bye(Math.round(least + Math.random() * (spreadMs - least))),
      ),
    );
  }
}
