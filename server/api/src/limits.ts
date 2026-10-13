import type { Context, MiddlewareHandler } from "hono";

import { ApiFailure, type Env } from "./http.ts";

/**
 * Counts requests per key in one-minute windows, in this process's memory. One API process
 * serves every request, so this is exact; with more processes it becomes a limit per process.
 */
export class RateLimiter {
  private readonly windows = new Map<
    string,
    { started: number; count: number }
  >();
  private readonly now: () => number;

  constructor(now: () => number = Date.now) {
    this.now = now;
  }

  allow(key: string, perMinute: number): boolean {
    const now = this.now();
    const window = this.windows.get(key);
    if (!window || now - window.started >= 60_000) {
      if (this.windows.size > 50_000) this.sweep(now);
      this.windows.set(key, { started: now, count: 1 });
      return true;
    }
    window.count += 1;
    return window.count <= perMinute;
  }

  /** Whether `key` is already past its limit, without counting this as another try. */
  exceeded(key: string, perMinute: number): boolean {
    const window = this.windows.get(key);
    return (
      window !== undefined &&
      this.now() - window.started < 60_000 &&
      window.count >= perMinute
    );
  }

  private sweep(now: number) {
    for (const [key, window] of this.windows) {
      if (now - window.started >= 60_000) this.windows.delete(key);
    }
  }
}

export function limit<E extends Env>(
  limiter: RateLimiter,
  name: string,
  perMinute: number,
  keyOf: (c: Parameters<MiddlewareHandler<E>>[0]) => string,
): MiddlewareHandler<E> {
  return async (c, next) => {
    if (!limiter.allow(`${name}:${keyOf(c)}`, perMinute)) {
      c.header("retry-after", "60");
      throw new ApiFailure(
        429,
        "rate_limited",
        "Too many requests; try again in a minute.",
      );
    }
    await next();
  };
}

/** The caller's address as Caddy saw it, so one phone's requests count together. */
export function addressOf(
  forwardedFor: string | string[] | undefined,
  socketAddress?: string,
): string {
  const header = Array.isArray(forwardedFor) ? forwardedFor[0] : forwardedFor;
  return header?.split(",")[0]?.trim() || socketAddress || "local";
}

export function clientAddress(c: Context): string {
  return addressOf(c.req.header("x-forwarded-for"));
}
