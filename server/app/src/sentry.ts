import * as Sentry from "@sentry/react";

import { config } from "./config.ts";

const TRACES_SAMPLE_RATE = 0.05;
const KEPT_HEADERS = ["User-Agent"];

/** Without its query and fragment, where sign-ins carry codes and a shared chat its key. */
const bare = (url: string) => url.replace(/[?#].*$/, "");

function scrub(fields: Record<string, unknown> | undefined) {
  if (!fields) return;
  for (const [key, value] of Object.entries(fields))
    if (typeof value === "string" && /url|^from$|^to$|href/i.test(key))
      fields[key] = bare(value);
}

export function startSentry(dsn: string) {
  Sentry.init({
    dsn,
    release: import.meta.env.VITE_SENTRY_RELEASE,
    environment: "production",
    dataCollection: {
      userInfo: false,
      cookies: false,
      httpHeaders: false,
      httpBodies: [],
      urlQueryParams: false,
    },
    // What the page writes to its console can hold anything Clerk or the API answered.
    integrations: (defaults) => [
      ...defaults.filter(({ name }) => name !== "Console"),
      Sentry.browserTracingIntegration(),
    ],
    tracesSampleRate: TRACES_SAMPLE_RATE,
    tracePropagationTargets: [config.apiUrl],
    beforeSend(event) {
      if (event.request) {
        const { url, headers = {} } = event.request;
        event.request = {
          ...(url && { url: bare(url) }),
          headers: Object.fromEntries(
            KEPT_HEADERS.flatMap((name) =>
              headers[name] ? [[name, headers[name]]] : [],
            ),
          ),
        };
      }
      if (event.user) event.user = event.user.id ? { id: event.user.id } : {};
      for (const crumb of event.breadcrumbs ?? []) scrub(crumb.data);
      return event;
    },
    beforeBreadcrumb(crumb) {
      scrub(crumb.data);
      return crumb;
    },
    beforeSendSpan(span) {
      span.name = bare(span.name);
      scrub(span.attributes);
      return span;
    },
  });
}

export const reportReactError = Sentry.reactErrorHandler();

/** The pseudonym the apps' usage pings give an account, so Sentry counts people without knowing who they are. */
async function accountHash(userId: string) {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(`sikemux-usage|${userId}`),
  );
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

let account: string | null = null;

export async function setAccount(userId: string | null) {
  account = userId;
  const id = userId && (await accountHash(userId));
  if (account === userId) Sentry.setUser(id ? { id } : null);
}
