import type {
  HookClientMessage,
  HookDelivery,
  HookHello,
  HookServerMessage,
} from "@sikemux/protocol";
import { HookCloseCode } from "@sikemux/protocol";
import { sql } from "kysely";
import type { WebSocket } from "ws";

import { hooksMessage } from "../devices/signature.ts";
import { hostAccounts } from "../devices/standing.ts";
import {
  ChallengedSocket,
  type SocketContext,
  type SocketKind,
} from "../sockets/socket.ts";
import type { HookHost, HookHosts } from "./hosts.ts";
import type { HookOptions } from "./options.ts";

interface HookContext extends SocketContext {
  hosts: HookHosts;
  options: HookOptions;
}

export const HOOK_SOCKET: SocketKind = {
  name: "hooks",
  incoming: "HookClientMessage",
  codes: HookCloseCode,
};

interface Host {
  userId: string;
  key: string;
}

/** Closes the connections of hosts no longer on their account, `userId`'s or everyone's. */
export async function recheckHookHosts(
  context: Pick<HookContext, "db" | "hosts">,
  userId?: string,
) {
  const hosts = context.hosts.of(userId);
  const standing = await hostAccounts(
    context.db,
    hosts.map((host) => host.key),
  );
  for (const host of hosts)
    if (standing.get(host.key) !== host.userId) host.removed();
}

/**
 * One host's connection for its webhooks: the requests they received, sent oldest first a few
 * ahead of the host's answers, and kept until it says it handled each.
 */
export class HookSocket extends ChallengedSocket<
  HookClientMessage,
  HookHello,
  Host
> {
  private readonly ahead = new Set<string>();
  private retryTimer: NodeJS.Timeout | undefined;
  private member: HookHost | undefined;

  private readonly context: HookContext;

  constructor(ws: WebSocket, ip: string, context: HookContext) {
    super(ws, ip, context, HOOK_SOCKET);
    this.context = context;
  }

  bye(reconnectAfterMs: number) {
    if (this.phase === "closing") return;
    this.send({ type: "bye", reconnectAfterMs });
    this.close(HookCloseCode.restarting, "restarting");
  }

  protected limits() {
    const { messagesPerSecond, maxMessageBytes } = this.context.options;
    return { perSecond: messagesPerSecond, bytes: maxMessageBytes };
  }

  private send(message: HookServerMessage) {
    this.write(message);
  }

  protected async prove(hello: HookHello): Promise<Host | undefined> {
    const userId = await this.provenHost(
      hello.key,
      hooksMessage(this.nonce, hello.key),
      hello.signature,
    );
    const host = { userId, key: hello.key };
    this.member = {
      ...host,
      wake: () => this.wake(host),
      checkIdle: () =>
        this.serially(async () => {
          await this.closeIfIdle(host);
        }),
      replaced: () =>
        this.close(HookCloseCode.replaced, "a newer connection for this host"),
      removed: () =>
        this.close(HookCloseCode.unauthenticated, "no longer on an account"),
      bye: (reconnectAfterMs) => this.bye(reconnectAfterMs),
    };
    this.context.hosts.admit(this.member);
    return host;
  }

  protected async opened(host: Host) {
    this.send({ type: "ready", heartbeatMs: this.context.options.heartbeatMs });
    if (await this.closeIfIdle(host)) return;
    await this.deliver(host);
  }

  protected handle(message: Exclude<HookClientMessage, HookHello>, host: Host) {
    if (message.type !== "done" || !this.ahead.delete(message.id)) return;
    this.serially(async () => {
      if (message.handled) await this.forget(host, message.id);
      else await this.later(host, message.id);
      await this.deliver(host);
    });
  }

  private wake(host: Host) {
    this.wakeOnce(() => this.deliver(host));
  }

  /** Closes as idle when the host has no webhooks and no requests waiting. */
  private async closeIfIdle(host: Host): Promise<boolean> {
    const { db } = this.context;
    const [hook, waiting] = await Promise.all([
      db
        .selectFrom("hooks")
        .select("id")
        .where("host_key", "=", host.key)
        .limit(1)
        .executeTakeFirst(),
      db
        .selectFrom("hook_deliveries")
        .select("id")
        .where("host_key", "=", host.key)
        .limit(1)
        .executeTakeFirst(),
    ]);
    if (hook || waiting) return false;
    this.close(HookCloseCode.idle, "no webhooks");
    return true;
  }

  private async deliver(host: Host) {
    const { db, options } = this.context;
    const room = options.deliveriesAhead - this.ahead.size;
    if (this.phase !== "open" || room <= 0) return;
    if (this.ws.bufferedAmount > options.dropBytes) {
      this.close(HookCloseCode.overloaded, "not reading");
      return;
    }
    const rows = await db
      .selectFrom("hook_deliveries")
      .select([
        "id",
        "hook_id",
        "received_at",
        "method",
        "headers",
        "query",
        "body",
      ])
      .where("host_key", "=", host.key)
      .where(
        "received_at",
        ">",
        sql<Date>`now() - make_interval(secs => ${options.keptMs / 1000})`,
      )
      .where((eb) =>
        eb.or([
          eb("retry_at", "is", null),
          eb("retry_at", "<=", sql<Date>`now()`),
        ]),
      )
      .$if(this.ahead.size > 0, (query) =>
        query.where("id", "not in", [...this.ahead]),
      )
      .orderBy("id")
      .limit(room)
      .execute();
    for (const row of rows) {
      if (this.phase !== "open") return;
      const delivery: HookDelivery = {
        id: row.id,
        hook: row.hook_id,
        receivedAt: row.received_at.toISOString(),
        method: row.method,
        headers: row.headers,
        query: row.query,
        body: row.body.toString("base64"),
      };
      this.ahead.add(row.id);
      this.send({ type: "delivery", delivery });
    }
    if (rows.length < room) await this.wakeForRetries(host);
  }

  /** Wakes the connection when the soonest request held back is due again. */
  private async wakeForRetries(host: Host) {
    const row = await this.context.db
      .selectFrom("hook_deliveries")
      .select((eb) => eb.fn.min("retry_at").as("due"))
      .where("host_key", "=", host.key)
      .where("retry_at", ">", sql<Date>`now()`)
      .executeTakeFirst();
    clearTimeout(this.retryTimer);
    if (!row?.due) return;
    const wait = Math.max(0, new Date(row.due).getTime() - Date.now());
    this.retryTimer = setTimeout(() => this.wake(host), wait);
  }

  private async forget(host: Host, id: string) {
    await this.context.db
      .deleteFrom("hook_deliveries")
      .where("id", "=", id)
      .where("host_key", "=", host.key)
      .execute();
  }

  /** Holds the request back for a while, longer each time the host could not handle it. */
  private async later(host: Host, id: string) {
    const { firstRetryMs, lastRetryMs } = this.context.options;
    await this.context.db
      .updateTable("hook_deliveries")
      .set((eb) => ({
        attempts: eb("attempts", "+", 1),
        retry_at: sql<Date>`now() + make_interval(secs => least(${lastRetryMs / 1000}, ${firstRetryMs / 1000} * power(2, attempts)))`,
      }))
      .where("id", "=", id)
      .where("host_key", "=", host.key)
      .execute();
  }

  protected closed() {
    clearTimeout(this.retryTimer);
    if (this.member) this.context.hosts.remove(this.member);
  }
}
