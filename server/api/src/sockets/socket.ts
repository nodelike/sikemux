import { randomBytes } from "node:crypto";
import { validator, type Definitions } from "@sikemux/protocol";
import type { Kysely } from "kysely";
import type { RawData, WebSocket } from "ws";

import { checkStanding, type Identity, type Verifier } from "../auth.ts";
import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";
import type { RateLimiter } from "../limits.ts";
import type { Logger } from "../log.ts";

/** The timers and limits every challenged connection shares. */
export interface SocketOptions {
  /** How long the challenge in the first message stays valid. */
  challengeMs: number;
  /** How long a new connection may stay silent before its hello. */
  helloTimeoutMs: number;
  heartbeatMs: number;
  /** How long without any message before a connection counts as dead. */
  silenceMs: number;
  /** How long a connection may outlive its token's expiry before it is closed. */
  authGraceMs: number;
  upgradesPerMinute: number;
  failedHellosPerMinute: number;
}

/** The close codes the live and share protocols both define. */
export interface CloseCodes {
  badMessage: number;
  unauthenticated: number;
  helloTimeout: number;
  overLimit: number;
  serverError: number;
  overloaded: number;
}

export interface SocketKind {
  /** Starts every rate-limit key of this kind of connection. */
  name: "live" | "share";
  incoming: keyof Definitions & string;
  codes: CloseCodes;
}

export interface SocketContext {
  db: Kysely<Tables>;
  verifier: Verifier;
  limiter: RateLimiter;
  log: Logger;
  options: SocketOptions;
}

export class HelloRefused extends Error {}
export class HelloLimited extends HelloRefused {}

const protocol = validator();

export function failedHelloKeys(
  kind: SocketKind["name"],
  ip: string,
  key?: string,
): string[] {
  return [
    `${kind}-failed-ip:${ip}`,
    ...(key ? [`${kind}-failed-key:${key}`] : []),
  ];
}

function isHello<Hello extends { type: "hello" }>(message: {
  type: string;
}): message is Hello {
  return message.type === "hello";
}

function sizeOf(data: RawData): number {
  if (Array.isArray(data))
    return data.reduce((total, part) => total + part.length, 0);
  return data instanceof ArrayBuffer ? data.byteLength : data.length;
}

/**
 * A connection that opens with a challenge, proves who it is in one hello, then trades
 * messages under a heartbeat and a per-second limit until it is closed.
 */
export abstract class ChallengedSocket<
  Incoming extends { type: string },
  Hello extends { type: "hello"; key?: string; role: string },
  Proven,
> {
  protected phase: "hello" | "proving" | "open" | "closing" = "hello";
  protected readonly nonce = randomBytes(32).toString("hex");
  protected readonly log: Logger;
  private readonly challengeExpires: number;
  private lastHeard = Date.now();
  private messageWindow = { started: 0, count: 0 };
  private proven: Proven | undefined;
  private queue: Promise<void> = Promise.resolve();
  private readonly timers: NodeJS.Timeout[] = [];
  private authTimer: NodeJS.Timeout | undefined;

  protected readonly ws: WebSocket;
  private readonly ip: string;
  private readonly base: SocketContext;
  private readonly kind: SocketKind;

  constructor(
    ws: WebSocket,
    ip: string,
    context: SocketContext,
    kind: SocketKind,
  ) {
    this.ws = ws;
    this.ip = ip;
    this.base = context;
    this.kind = kind;
    this.challengeExpires = Date.now() + context.options.challengeMs;
    this.log = context.log.child({ [kind.name]: true });
  }

  /** The most messages a second, and the largest one, this connection may send. */
  protected abstract limits(): { perSecond: number; bytes: number };
  /** Who the hello proves the connection is; throws HelloRefused, or closes and returns nothing. */
  protected abstract prove(hello: Hello): Promise<Proven | undefined>;
  protected abstract opened(proven: Proven): Promise<void> | void;
  protected abstract handle(
    message: Exclude<Incoming, Hello>,
    proven: Proven,
  ): void;
  protected abstract closed(
    code: number,
    proven: Proven | undefined,
  ): Promise<void> | void;

  start(timers: NodeJS.Timeout[] = []) {
    const { options } = this.base;
    this.ws.on("message", (data, isBinary) => this.onMessage(data, isBinary));
    this.ws.on("close", (code) => void this.onClose(code));
    this.ws.on("error", (error) =>
      this.log.debug({ err: error }, `a ${this.kind.name} connection failed`),
    );
    this.write({
      type: "challenge",
      nonce: this.nonce,
      expiresAt: new Date(this.challengeExpires).toISOString(),
    });
    this.timers.push(
      ...timers,
      setTimeout(() => {
        if (this.phase === "hello")
          this.close(this.kind.codes.helloTimeout, "no hello in time");
      }, options.helloTimeoutMs),
      setInterval(() => {
        if (Date.now() - this.lastHeard > options.silenceMs) {
          this.ws.terminate();
          return;
        }
        if (this.phase === "open") this.write({ type: "ping" });
      }, options.heartbeatMs),
    );
  }

  protected write(message: object) {
    if (this.ws.readyState === this.ws.OPEN)
      this.ws.send(JSON.stringify(message));
  }

  protected close(code: number, reason: string) {
    if (this.phase === "closing") return;
    this.phase = "closing";
    this.ws.close(code, reason);
  }

  protected serially(task: () => Promise<void>) {
    this.queue = this.queue.then(task).catch((error: unknown) => {
      this.log.error(
        { err: error },
        `a ${this.kind.name} connection's work failed`,
      );
      this.close(this.kind.codes.serverError, "server error");
    });
  }

  protected refuse(why: string) {
    this.close(this.kind.codes.badMessage, why);
  }

  private onMessage(data: RawData, isBinary: boolean) {
    const now = Date.now();
    this.lastHeard = now;
    const { codes } = this.kind;
    const limits = this.limits();
    if (sizeOf(data) > limits.bytes) {
      this.close(codes.badMessage, "message too large");
      return;
    }
    if (now - this.messageWindow.started >= 1_000)
      this.messageWindow = { started: now, count: 0 };
    this.messageWindow.count += 1;
    if (this.messageWindow.count > limits.perSecond) {
      this.close(codes.overLimit, "too many messages");
      return;
    }
    if (this.phase === "closing") return;

    let value: unknown;
    try {
      value = isBinary ? undefined : JSON.parse(data.toString());
    } catch {
      value = undefined;
    }
    const result = protocol.validate(this.kind.incoming, value);
    if (!result.ok) {
      this.close(codes.badMessage, `not a ${this.kind.name} message`);
      return;
    }
    const message = result.value as Hello | Exclude<Incoming, Hello>;
    if (isHello<Hello>(message)) {
      if (this.phase !== "hello")
        return this.refuse("one hello per connection");
      this.phase = "proving";
      this.serially(() => this.proveHello(message));
      return;
    }
    if (this.phase !== "open" || !this.proven)
      return this.refuse("hello first");
    if (message.type === "pong") return;
    this.handle(message, this.proven);
  }

  private async proveHello(hello: Hello) {
    const { limiter, options } = this.base;
    try {
      if (Date.now() > this.challengeExpires)
        throw new HelloRefused("the challenge expired");
      if (
        hello.key &&
        limiter.exceeded(
          `${this.kind.name}-failed-key:${hello.key}`,
          options.failedHellosPerMinute,
        )
      )
        throw new HelloLimited("too many failed hellos for this key");
      const proven = await this.prove(hello);
      if (!proven) return;
      this.proven = proven;
      if (this.phase === "closing") return;
      this.phase = "open";
      await this.opened(proven);
    } catch (error) {
      if (!(error instanceof HelloRefused || error instanceof ApiFailure))
        throw error;
      for (const key of failedHelloKeys(this.kind.name, this.ip, hello.key))
        limiter.allow(key, options.failedHellosPerMinute);
      this.log.info(
        { role: hello.role, key: hello.key?.slice(0, 8), why: error.message },
        `refused a ${this.kind.name} hello`,
      );
      if (error instanceof HelloLimited)
        this.close(this.kind.codes.overLimit, "too many failed hellos");
      else this.close(this.kind.codes.unauthenticated, "not signed in");
    }
  }

  /** Closes the connection once a sign-in that expires at `expiresAt` has run out. */
  protected watchExpiry(expiresAt: Date) {
    clearTimeout(this.authTimer);
    const delay =
      expiresAt.getTime() - Date.now() + this.base.options.authGraceMs;
    this.authTimer = setTimeout(
      () => this.close(this.kind.codes.unauthenticated, "the sign-in expired"),
      Math.max(0, delay),
    );
  }

  /** Takes a fresh token for the same account, which `accepts` must allow, and extends the sign-in. */
  protected reauthenticate(
    token: string,
    userId: string,
    accepts: (identity: Identity) => boolean,
  ) {
    this.serially(async () => {
      const { verifier, db } = this.base;
      try {
        const identity = await verifier.verify(token);
        if (!accepts(identity))
          throw new HelloRefused("this connection does not renew that token");
        if (identity.userId !== userId)
          throw new HelloRefused("the token is for another account");
        await checkStanding(db, identity);
        this.watchExpiry(identity.expiresAt);
      } catch (error) {
        if (!(error instanceof HelloRefused || error instanceof ApiFailure))
          throw error;
        this.close(this.kind.codes.unauthenticated, "not signed in");
      }
    });
  }

  /** Waits for the work already queued, as a close does before recording anything. */
  protected settled(): Promise<void> {
    return this.queue;
  }

  private async onClose(code: number) {
    this.phase = "closing";
    for (const timer of this.timers) clearTimeout(timer);
    clearTimeout(this.authTimer);
    await this.closed(code, this.proven);
  }
}
