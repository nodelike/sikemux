import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import * as Sentry from "@sentry/node";

import { version } from "./version.ts";

const TRACES_SAMPLE_RATE = 0.05;
/** The services the API calls itself; a stashed link's preview fetches a person's own URL, which stays out. */
const SERVICE_HOSTS =
  /(^|\.)(sikemux\.com|clerk\.com|apple\.com|googleapis\.com)$/;
const PRIVATE_KEY =
  /authorization|cookie|token|secret|password|email|address|query|connection_string|^ip$/i;
const HOOK_TOKEN = /(\/v1\/hooks\/[^/?#]+\/)[^/?#]+/g;
const KEPT_HEADERS = ["user-agent", "content-type", "content-length"];

/** The pseudonym the apps' usage pings give an account, so Sentry counts people without knowing who they are. */
const accountHash = (userId: string) =>
  createHash("sha256").update(`sikemux-usage|${userId}`).digest("hex");

const withoutHookToken = (text: string) =>
  text.includes("/v1/hooks/") ? text.replace(HOOK_TOKEN, "$1[token]") : text;

function scrub(fields: Record<string, unknown> | undefined) {
  if (!fields) return;
  for (const [key, value] of Object.entries(fields)) {
    if (key === "userId" && typeof value === "string")
      fields[key] = accountHash(value);
    else if (PRIVATE_KEY.test(key)) Reflect.deleteProperty(fields, key);
    else if (typeof value === "string") fields[key] = withoutHookToken(value);
  }
}

function scrubEvent<E extends Sentry.Event>(event: E): E {
  if (event.request) {
    const { method, url, headers = {} } = event.request;
    event.request = {
      ...(method && { method }),
      ...(url && { url: withoutHookToken(url.replace(/[?#].*$/, "")) }),
      headers: Object.fromEntries(
        KEPT_HEADERS.flatMap((name) =>
          headers[name] ? [[name, headers[name]]] : [],
        ),
      ),
    };
  }
  if (event.transaction)
    event.transaction = withoutHookToken(event.transaction);
  if (event.user) event.user = event.user.id ? { id: event.user.id } : {};
  scrub(event.contexts?.trace?.data);
  scrub(event.contexts?.pino);
  for (const crumb of event.breadcrumbs ?? []) scrub(crumb.data);
  return event;
}

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
      span.name = withoutHookToken(span.name);
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

/** Sends what is queued before the process exits. */
export const flushSentry = () => Sentry.close(2000);
