import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import * as Sentry from "@sentry/node";

import { version } from "./version.ts";

const TRACES_SAMPLE_RATE = 0.05;
/** The services the API calls itself; a stashed link's preview fetches a person's own URL, which stays out. */
const SERVICE_HOSTS =
  /(^|\.)(sikemux\.com|clerk\.com|apple\.com|googleapis\.com)$/;
const PRIVATE_KEY =
  /authorization|cookie|token|secret|password|email|address|query|connection_string|^ip$|x-page-proof|x-page-title|x-page-shared-by|proof/i;
const HOOK_TOKEN = /(\/v1\/hooks\/[^/?#]+\/)[^/?#]+/g;
const SHARE_ID = /(\/v1\/(?:pages|shares)\/)[^/?#]+/g;
const KEPT_HEADERS = ["user-agent", "content-type", "content-length"];

/** The pseudonym the apps' usage pings give an account, so Sentry counts people without knowing who they are. */
const accountHash = (userId: string) =>
  createHash("sha256").update(`sikemux-usage|${userId}`).digest("hex");

const scrubText = (text: string) => {
  let out = text;
  if (out.includes("/v1/hooks/")) out = out.replace(HOOK_TOKEN, "$1[token]");
  if (out.includes("/v1/pages/") || out.includes("/v1/shares/"))
    out = out.replace(SHARE_ID, "$1[id]");
  return out;
};

const MAX_SCRUB_DEPTH = 10;

function scrubValue(
  value: unknown,
  seen: WeakSet<object>,
  depth: number,
): unknown {
  if (typeof value === "string") return scrubText(value);
  if (!value || typeof value !== "object" || depth >= MAX_SCRUB_DEPTH)
    return value;
  if (seen.has(value)) return value;
  seen.add(value);
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i += 1)
      value[i] = scrubValue(value[i], seen, depth + 1);
    return value;
  }
  scrub(value as Record<string, unknown>, seen, depth + 1);
  return value;
}

function scrub(
  fields: Record<string, unknown> | undefined,
  seen = new WeakSet<object>(),
  depth = 0,
) {
  if (!fields || depth >= MAX_SCRUB_DEPTH) return;
  seen.add(fields);
  for (const [key, value] of Object.entries(fields)) {
    if (key === "userId" && typeof value === "string")
      fields[key] = accountHash(value);
    else if (PRIVATE_KEY.test(key)) Reflect.deleteProperty(fields, key);
    else fields[key] = scrubValue(value, seen, depth);
  }
}

function scrubEvent<E extends Sentry.Event>(event: E): E {
  if (event.request) {
    const { method, url, headers = {} } = event.request;
    event.request = {
      ...(method && { method }),
      ...(url && { url: scrubText(url.replace(/[?#].*$/, "")) }),
      headers: Object.fromEntries(
        KEPT_HEADERS.flatMap((name) =>
          headers[name] ? [[name, headers[name]]] : [],
        ),
      ),
    };
  }
  if (event.transaction) event.transaction = scrubText(event.transaction);
  if (event.message) event.message = scrubText(event.message);
  if (event.user) event.user = event.user.id ? { id: event.user.id } : {};
  scrub(event.extra);
  if (event.contexts) {
    for (const ctx of Object.values(event.contexts))
      if (ctx && typeof ctx === "object") scrub(ctx);
  }
  for (const exception of event.exception?.values ?? []) {
    if (exception.value) exception.value = scrubText(exception.value);
    scrub(exception.mechanism?.data);
  }
  for (const crumb of event.breadcrumbs ?? []) {
    if (crumb.message) crumb.message = scrubText(crumb.message);
    scrub(crumb.data);
  }
  return event;
}

/** A log line's `fingerprint` makes its own issue; before Dedupe, which drops a repeat of the last message. */
const fingerprintFromLog = {
  name: "FingerprintFromLog",
  processEvent(event: Sentry.Event) {
    const pino = event.contexts?.pino;
    if (Array.isArray(pino?.fingerprint)) {
      event.fingerprint = pino.fingerprint.map(String);
      Reflect.deleteProperty(pino, "fingerprint");
    }
    return event;
  },
};

/**
 * Reports errors, a sample of requests and the logs to Sentry. Only citadel sets SENTRY_DSN,
 * so dev servers and tests send nothing.
 */
export function startSentry(env: NodeJS.ProcessEnv) {
  const dsn = env.SENTRY_DSN?.trim();
  if (!dsn) return;
  const ignoreOutgoingRequests = (url: string) => {
    try {
      return !SERVICE_HOSTS.test(new URL(url).hostname);
    } catch {
      return true;
    }
  };
  Sentry.init({
    dsn,
    release: version,
    environment: "production",
    dataCollection: {
      userInfo: false,
      cookies: false,
      httpHeaders: { request: { allow: KEPT_HEADERS }, response: false },
      httpBodies: [],
      urlQueryParams: false,
      databaseQueryData: false,
      queues: false,
      stackFrameVariables: false,
    },
    tracesSampleRate: TRACES_SAMPLE_RATE,
    tracePropagationTargets: [],
    integrations: (defaults) => [
      fingerprintFromLog,
      // It pauses on every exception in a debugger to read variables, which are never sent.
      ...defaults.filter(({ name }) => name !== "LocalVariablesAsync"),
      // Paths from the API's folder, the same in every release's folder on citadel.
      Sentry.rewriteFramesIntegration({
        root: fileURLToPath(new URL("..", import.meta.url)),
      }),
      Sentry.httpIntegration({ ignoreOutgoingRequests }),
      Sentry.nativeNodeFetchIntegration({ ignoreOutgoingRequests }),
      Sentry.pinoIntegration({
        log: { levels: ["info", "warn", "error", "fatal"] },
        error: { levels: ["error", "fatal"] },
      }),
    ],
    beforeSend: scrubEvent,
    beforeSendSpan(span) {
      span.name = scrubText(span.name);
      scrub(span.attributes);
      return span;
    },
    beforeBreadcrumb(crumb) {
      scrub(crumb.data);
      return crumb;
    },
    beforeSendLog(log) {
      // Each request already has its span, and a line for every one would cost the most.
      if (log.message === "request") return null;
      scrub(log.attributes);
      return log;
    },
  });
}

/** Marks what the rest of this request reports as the signed-in account's, by its pseudonym. */
export function reportAs(userId: string) {
  if (Sentry.isEnabled()) Sentry.setUser({ id: accountHash(userId) });
}

/** Raises an issue of its own for one account, named only by its pseudonym, for staff to look at. */
export function alertStaff(
  userId: string,
  what: string,
  detail?: Record<string, unknown>,
) {
  if (!Sentry.isEnabled()) return;
  const account = accountHash(userId);
  Sentry.captureMessage(what, {
    level: "error",
    fingerprint: [what, account],
    user: { id: account },
    ...(detail && { extra: detail }),
  });
}

/** Sends what is queued before the process exits. */
export const flushSentry = () => Sentry.close(2000);
