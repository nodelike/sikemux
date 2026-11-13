import type { LivePush, PushResult } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";
import type { Delivery, PushProviders } from "./provider.ts";

interface PushLimits {
  /** Pushes a minute from one host to one phone. */
  perPair: number;
  /** Pushes a minute to one phone, from all its hosts together. */
  perPhone: number;
  /** Waits before each retry of a push the platform asked to have retried. */
  retryDelaysMs: readonly number[];
  /** A push is given up this long after it arrived, even if it has not expired. */
  giveUpMs: number;
  /** The longest Retry-After the sender honours; a longer one gives up instead. */
  maxRetryAfterMs: number;
  /** How many pushes, by id, are remembered so one sent again is answered without going out twice. */
  remembered: number;
}

const PUSH_LIMITS: PushLimits = {
  perPair: 30,
  perPhone: 120,
  retryDelaysMs: [1_000, 2_000, 4_000],
  giveUpMs: 60_000,
  maxRetryAfterMs: 30_000,
  remembered: 20_000,
};

/** Results worth trying again rather than answering a repeat with. */
const PASSING: ReadonlySet<PushResult> = new Set(["throttled", "failed"]);

/** A token that failed this many times in a row, with no success in 30 days, is deleted. */
const FAILURES_BEFORE_FORGETTING = 20;

interface Sender {
  userId: string;
  key: string;
}

export interface PusherOptions {
  db: Kysely<Tables>;
  log: Logger;
  limiter: RateLimiter;
  providers: PushProviders;
  limits?: Partial<PushLimits>;
  now?: () => number;
  sleep?: (ms: number) => Promise<void>;
}

const sleep = (ms: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, ms));

/** Passes hosts' sealed notifications to the platform that reaches each phone. */
export class Pusher {
  private readonly db: Kysely<Tables>;
  private readonly log: Logger;
  private readonly limiter: RateLimiter;
  private readonly providers: PushProviders;
  private readonly limits: PushLimits;
  private readonly now: () => number;
  private readonly sleep: (ms: number) => Promise<void>;
  /** By sender and push id, oldest first, until each push expires. */
  private readonly answered = new Map<
    string,
    { result: Promise<PushResult>; expiresAt: number }
  >();

  constructor(options: PusherOptions) {
    this.db = options.db;
    this.log = options.log.child({ push: true });
    this.limiter = options.limiter;
    this.providers = options.providers;
    this.limits = { ...PUSH_LIMITS, ...options.limits };
    this.now = options.now ?? Date.now;
    this.sleep = options.sleep ?? sleep;
  }

  /**
   * A host that lost its connection before the answer sends the push again with the same id;
   * that gets the first one's answer instead of reaching the phone twice.
   */
  push(sender: Sender, push: LivePush): Promise<PushResult> {
    if (!push.id) return this.deliver(sender, push);
    const id = `${sender.key}:${push.id}`;
    const earlier = this.answered.get(id);
    if (earlier && earlier.expiresAt > this.now()) {
      this.log.info(
        { from: sender.key.slice(0, 8), to: push.to.slice(0, 8) },
        "pushed already",
      );
      return earlier.result;
    }
    const result = this.deliver(sender, push);
    this.answered.delete(id);
    this.answered.set(id, { result, expiresAt: Date.parse(push.expiresAt) });
    for (const oldest of this.answered.keys()) {
      if (this.answered.size <= this.limits.remembered) break;
      this.answered.delete(oldest);
    }
    const forget = () => {
      if (this.answered.get(id)?.result === result) this.answered.delete(id);
    };
    result.then((answer) => {
      if (PASSING.has(answer)) forget();
    }, forget);
    return result;
  }

  private async deliver(sender: Sender, push: LivePush): Promise<PushResult> {
    const started = this.now();
    const expiresAt = Date.parse(push.expiresAt);
    const facts = {
      from: sender.key.slice(0, 8),
      to: push.to.slice(0, 8),
      kind: push.kind,
      bytes: push.blob.length,
    };
    const finish = (
      result: PushResult,
      detail: Record<string, unknown> = {},
    ) => {
      this.log.info(
        { ...facts, ...detail, result, ms: this.now() - started },
        "pushed",
      );
      return result;
    };

    if (!(expiresAt > started)) return finish("expired");

    const target = await this.target(sender, push.to);
    if (!target) return finish("not_allowed");
    const { token, platform } = target;
    if (!token || !platform) return finish("no_token");

    if (
      !this.limiter.allow(
        `push-pair:${sender.key}:${push.to}`,
        this.limits.perPair,
      ) ||
      !this.limiter.allow(`push-phone:${push.to}`, this.limits.perPhone)
    )
      return finish("throttled", { platform });

    const provider = this.providers[platform];
    if (!provider) return finish("not_set_up", { platform });
    const activity = push.kind === "activity" || push.kind === "activity_end";
    const activityToken = activity ? push.activityToken : undefined;
    if (activity && (platform !== "apns" || !activityToken))
      return finish("not_set_up", { platform });
    const to = activityToken ?? token;

    const giveUpAt = Math.min(expiresAt, started + this.limits.giveUpMs);
    let delivery: Delivery;
    let attempt = 0;
    for (;;) {
      delivery = await provider.send({
        token: to,
        kind: push.kind,
        sentAt: started,
        collapseId: push.collapseId,
        blob: push.blob,
        ttlSeconds: (expiresAt - this.now()) / 1000,
        ...(target.apns_environment
          ? { apnsEnvironment: target.apns_environment }
          : {}),
      });
      if (delivery.outcome !== "retry") break;
      const wait =
        delivery.afterMs ?? this.limits.retryDelaysMs[attempt] ?? Infinity;
      if (
        attempt >= this.limits.retryDelaysMs.length ||
        wait > this.limits.maxRetryAfterMs ||
        this.now() + wait >= giveUpAt
      )
        break;
      attempt += 1;
      await this.sleep(wait);
    }

    const detail = {
      platform,
      attempts: attempt + 1,
      ...(delivery.outcome === "sent" ? {} : { why: delivery.reason }),
    };
    if (activity)
      return finish(
        delivery.outcome === "sent"
          ? "sent"
          : delivery.outcome === "dead"
            ? "no_token"
            : "failed",
        detail,
      );
    switch (delivery.outcome) {
      case "sent":
        await this.db
          .updateTable("push_tokens")
          .set({ last_ok_at: sql<Date>`now()`, failures: 0 })
          .where("device_key", "=", push.to)
          .where("token", "=", token)
          .execute();
        return finish("sent", detail);
      case "dead":
        await this.db
          .deleteFrom("push_tokens")
          .where("device_key", "=", push.to)
          .where("token", "=", target.token)
          .execute();
        return finish("no_token", { ...detail, forgot: true });
      case "retry":
      case "failed":
        await this.countFailure(push.to, token);
        return finish("failed", detail);
    }
  }

  /** The phone's token, when the sender is a host and the phone a client on one account. */
  private async target(sender: Sender, phone: string) {
    const row = await this.db
      .selectFrom("devices as phone")
      .leftJoin("push_tokens", "push_tokens.device_key", "phone.key")
      .select([
        "push_tokens.platform",
        "push_tokens.token",
        "push_tokens.apns_environment",
      ])
      .where("phone.key", "=", phone)
      .where("phone.role", "=", "client")
      .where("phone.user_id", "=", sender.userId)
      .where(({ exists, selectFrom }) =>
        exists(
          selectFrom("devices as host")
            .select("host.key")
            .where("host.key", "=", sender.key)
            .where("host.role", "=", "host")
            .where("host.user_id", "=", sender.userId),
        ),
      )
      .executeTakeFirst();
    return row;
  }

  private async countFailure(phone: string, token: string) {
    await this.db.transaction().execute(async (trx) => {
      await trx
        .updateTable("push_tokens")
        .set({ failures: sql<number>`failures + 1` })
        .where("device_key", "=", phone)
        .where("token", "=", token)
        .execute();
      await trx
        .deleteFrom("push_tokens")
        .where("device_key", "=", phone)
        .where("failures", ">=", FAILURES_BEFORE_FORGETTING)
        .where((eb) =>
          eb.or([
            eb("last_ok_at", "is", null),
            eb("last_ok_at", "<", sql<Date>`now() - interval '30 days'`),
          ]),
        )
        .execute();
    });
  }
}
