import type { Logger } from "./log.ts";

/** Failed calls to another service are retried after a minute, doubling up to an hour. */
const RETRY_BASE_MS = 60_000;
const RETRY_MAX_MS = 60 * 60_000;
/** After this many failures every further one is logged as an error, which alerts. */
const ALERT_AFTER_ATTEMPTS = 6;

/**
 * After a call to another service failed for the attempts-th time: logs it, as an error once it
 * keeps failing, and answers when to try again.
 */
export function retryLater(
  log: Logger,
  { service, what }: { service: string; what: string },
  {
    error,
    attempts,
    ...fields
  }: Record<string, unknown> & { error: unknown; attempts: number },
): Date {
  const logged = { err: error, ...fields, attempts };
  if (attempts >= ALERT_AFTER_ATTEMPTS)
    log.error(logged, `${service} still cannot ${what}`);
  else log.warn(logged, `${service} could not ${what}; retrying later`);
  const delay = Math.min(
    RETRY_MAX_MS,
    RETRY_BASE_MS * 2 ** Math.max(0, attempts - 1),
  );
  return new Date(Date.now() + delay);
}
