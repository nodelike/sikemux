import type { Delivery } from "./provider.ts";

/** A Retry-After header's seconds, as milliseconds. */
export function retryAfterMs(
  header: string | null | undefined,
): number | undefined {
  const seconds = header == null ? NaN : Number(header);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds * 1000 : undefined;
}

/** Busy or failing on its side: the same request may work later. */
export function worthRetrying(status: number): boolean {
  return status === 429 || status >= 500;
}

/** A request the push service never answered is worth trying again. */
export function unanswered(error: Error): Delivery {
  return { outcome: "retry", reason: error.message };
}

/**
 * What a push service's refusal means once the provider has picked out dead tokens: 429 and
 * 5xx are worth trying again, after Retry-After when it says, and anything else failed.
 */
export function refused(
  status: number,
  reason: string,
  retryAfter: string | null | undefined,
): Delivery {
  if (worthRetrying(status)) {
    const afterMs = retryAfterMs(retryAfter);
    return {
      outcome: "retry",
      reason,
      ...(afterMs === undefined ? {} : { afterMs }),
    };
  }
  return { outcome: "failed", reason: `${status} ${reason}` };
}
