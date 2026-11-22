import { createHash, timingSafeEqual } from "node:crypto";
import { Hono, type MiddlewareHandler } from "hono";

import type { Services } from "../app.ts";
import { ApiFailure, readBody, type Env } from "../http.ts";
import { clientAddress, limit } from "../limits.ts";
import type { Logger } from "../log.ts";
import type { ClerkBackend, DemoAccount } from "./clerk.ts";
import { giveNoStanding } from "./proofs.ts";

const HOUR_MS = 60 * 60_000;
/** Wrong passwords in an hour past which staff are told, once that hour. */
const WRONG_PER_HOUR = 50;
const PER_CALLER_PER_MINUTE = 5;
/** Sign-ins a minute for the demo's own address, from everyone together. */
const DEMO_PER_MINUTE = 30;
const TICKET_SECONDS = 120;
const RETRY_LOAD_MS = 60_000;

const sha256 = (text: string) =>
  createHash("sha256").update(text.trim().toLowerCase()).digest();

/** What of the demo account staff want to hear about when it changes. */
const shapeOf = (demo: DemoAccount | null) =>
  JSON.stringify(
    demo && {
      address: demo.address,
      addresses: [...demo.addresses].sort(),
      passwordSetAt: demo.passwordSetAt,
      secondFactor: demo.secondFactor,
    },
  );

/**
 * The store reviewers' demo account: the one Clerk user REVIEW_ACCOUNT_USER_ID names, while it
 * carries the demo mark. It signs in with its password and no emailed code, and its addresses
 * give no standing at their domain.
 */
export class ReviewAccount {
  readonly userId: string;
  private demo: { hash: Buffer; addressSha256: string } | null = null;
  /** What staff are told about when it changes; undefined until Clerk first answered. */
  private shape: string | undefined;
  private readonly now: () => number;
  /** How long every answer takes at least, so none tells a wrong address from a wrong password. */
  private readonly answerAfterMs: number;
  private wrong = 0;
  private wrongSince = 0;
  /** Reads of Clerk started, and the newest one applied, so a slower older read never overwrites a newer one. */
  private reads = 0;
  private applied = 0;

  constructor(userId: string, { now = Date.now, answerAfterMs = 1000 } = {}) {
    this.userId = userId;
    this.now = now;
    this.answerAfterMs = answerAfterMs;
  }

  get addressSha256(): string | null {
    return this.demo?.addressSha256 ?? null;
  }

  /** Reads the account from Clerk again; staff hear when it changed since the last read. */
  async load(clerk: ClerkBackend, log: Logger): Promise<void> {
    const read = ++this.reads;
    const demo = await clerk.demoAccount(this.userId);
    if (read < this.applied) return;
    this.applied = read;
    giveNoStanding(demo?.addresses ?? []);
    const shape = shapeOf(demo);
    if (this.shape !== undefined && this.shape !== shape)
      log.error(
        "the store reviewers' demo account changed in Clerk; check App Review can still sign in",
      );
    this.shape = shape;
    if (!demo)
      log.warn(
        "REVIEW_ACCOUNT_USER_ID names no Clerk user with the demo mark and a verified address",
      );
    const hash = demo && sha256(demo.address);
    this.demo = hash && { hash, addressSha256: hash.toString("hex") };
  }

  /** Loads the account, trying again each minute until Clerk answers. */
  keepLoading(clerk: ClerkBackend, log: Logger) {
    this.load(clerk, log).catch((error: unknown) => {
      log.warn({ err: error }, "reading the demo account from Clerk failed");
      setTimeout(() => this.keepLoading(clerk, log), RETRY_LOAD_MS).unref();
    });
  }

  get ready(): boolean {
    return this.demo !== null;
  }

  isDemo(email: string): boolean {
    return this.demo !== null && timingSafeEqual(sha256(email), this.demo.hash);
  }

  /**
   * A sign-in token for the demo account, or null when the password is wrong or Clerk failed.
   * Never log the password.
   */
  async ticket(
    clerk: ClerkBackend,
    password: string,
    log: Logger,
  ): Promise<string | null> {
    try {
      if (await clerk.passwordMatches(this.userId, password))
        return await clerk.signInToken(this.userId, TICKET_SECONDS);
      this.countWrong(log);
    } catch (error) {
      log.error({ err: error }, "the demo account's sign-in failed");
    }
    return null;
  }

  private countWrong(log: Logger) {
    if (this.now() - this.wrongSince >= HOUR_MS) {
      this.wrong = 0;
      this.wrongSince = this.now();
    }
    this.wrong += 1;
    if (this.wrong === WRONG_PER_HOUR + 1)
      log.error(
        "the demo account's password was guessed wrong more than 50 times in an hour",
      );
  }

  /** Runs `work`, then waits out what is left of the least time an answer takes. */
  async answering<T>(work: () => Promise<T>): Promise<T> {
    const started = this.now();
    try {
      return await work();
    } finally {
      const left = started + this.answerAfterMs - this.now();
      if (left > 0) await new Promise((done) => setTimeout(done, left));
    }
  }
}

const REFUSED = () =>
  new ApiFailure(
    401,
    "unauthorized",
    "That email and password don't match the demo account.",
  );

const TOO_MANY = () =>
  new ApiFailure(
    429,
    "rate_limited",
    "Too many requests; try again in a minute.",
  );

/**
 * Signs in the store reviewers' demo account without the emailed code Clerk asks a new device
 * for. With no demo account set up, both routes answer as if they did not exist.
 */
export function reviewSignInRoutes({ review, clerk, limiter }: Services) {
  const isSet: MiddlewareHandler<Env> = async (c, next) => {
    if (!review?.ready || !clerk)
      throw new ApiFailure(
        404,
        "not_found",
        `No route matches ${c.req.method} ${c.req.path}.`,
      );
    await next();
  };
  return new Hono<Env>()
    .use(isSet)
    .get("/", limit<Env>(limiter, "review-account", 30, clientAddress), (c) =>
      c.json({ addressSha256: review?.addressSha256 }),
    )
    .post(
      "/",
      limit<Env>(
        limiter,
        "review-sign-in",
        PER_CALLER_PER_MINUTE,
        clientAddress,
      ),
      async (c) => {
        const { email, password } = await readBody(c, "ReviewSignInRequest");
        if (!review || !clerk) throw REFUSED();
        const ticket = await review.answering(async () => {
          if (!review.isDemo(email)) return null;
          if (!limiter.allow("review-sign-in-demo", DEMO_PER_MINUTE)) {
            c.header("retry-after", "60");
            throw TOO_MANY();
          }
          return review.ticket(clerk, password, c.get("log"));
        });
        if (!ticket) throw REFUSED();
        c.get("log").info("the demo account signed in");
        return c.json({ ticket });
      },
    );
}
