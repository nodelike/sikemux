import type { Network } from "@sikemux/protocol";
import { Hono } from "hono";
import { cors } from "hono/cors";

import type { AppleSignIn } from "./account/apple.ts";
import { profileRoutes } from "./account/profile.ts";
import type { ClerkBackend } from "./account/clerk.ts";
import { accountRoutes } from "./account/routes.ts";
import type { Verifier } from "./auth.ts";
import type { Database } from "./db.ts";
import { deviceRoutes } from "./devices/routes.ts";
import { healthRoutes } from "./health/routes.ts";
import type { HookHosts } from "./hooks/hosts.ts";
import { hookReceiveRoutes, hookRoutes } from "./hooks/routes.ts";
import { hostSignInRoutes } from "./host/routes.ts";
import {
  ApiFailure,
  errorResponse,
  limitBody,
  requestContext,
  type Env,
} from "./http.ts";
import type { JoinSigner } from "./join/signer.ts";
import { clientAddress, limit, type RateLimiter } from "./limits.ts";
import type { Logger } from "./log.ts";
import { networkRoutes } from "./network/routes.ts";
import type { Places } from "./places.ts";
import type { PushSettings } from "./push/settings.ts";
import { relayAccessRoutes, type RelayAccess } from "./relay/access.ts";
import type { StashFiles } from "./stash/files.ts";
import { stashRoutes } from "./stash/routes.ts";
import { updateRoutes } from "./updates/routes.ts";
import { clerkWebhookRoutes } from "./webhooks/clerk.ts";

export interface Services {
  database: Database;
  log: Logger;
  appOrigin: string;
  /** The API's own public origin, which webhook addresses start with. */
  apiOrigin: string;
  verifier: Verifier;
  limiter: RateLimiter;
  relayAccess: RelayAccess;
  /** Clerk's Backend API, or null when CLERK_SECRET_KEY is not set. */
  clerk: ClerkBackend | null;
  /** Apple's token endpoints, or null when APPLE_SIGNIN_KEY_FILE is not set. */
  appleSignIn: AppleSignIn | null;
  /** The secret Clerk signs webhooks with, or null when CLERK_WEBHOOK_SECRET is not set. */
  webhookSecret: string | null;
  network: Network;
  push: Pick<PushSettings, "app" | "allowSandbox">;
  join: JoinSigner;
  /** Where hosts asking to sign in are. */
  places: Places;
  /** What the relay sends to ask whether a device may use it, or null when it is not set up. */
  relayAccessToken: string | null;
  /** Where stashed files are kept, or null when the stash is not set up. */
  stash: StashFiles | null;
  /** The hosts connected for their webhooks, which hear at once of a request for them. */
  hookHosts: HookHosts;
}

export function createApp(services: Services) {
  const {
    log,
    appOrigin,
    limiter,
    relayAccess,
    relayAccessToken,
    network,
    database,
    webhookSecret,
  } = services;
  const app = new Hono<Env>();

  app.use(requestContext(log));
  app.route("/internal", relayAccessRoutes(relayAccess, relayAccessToken));
  // Senders such as GitHub post every account's webhooks from a few addresses, so each webhook
  // has its own limit instead of the address's.
  app.route("/v1/hooks", hookReceiveRoutes(services));
  app.use(limit<Env>(limiter, "address", 600, clientAddress));
  app.use(
    cors({
      origin: appOrigin,
      allowMethods: ["GET", "POST", "PUT", "PATCH", "DELETE"],
      allowHeaders: ["authorization", "content-type"],
      exposeHeaders: ["x-request-id"],
      maxAge: 600,
    }),
  );
  // The stash takes files, so it limits its own bodies.
  app.route("/v1/stash", stashRoutes(services));
  app.use(limitBody());

  app.route("/v1/health", healthRoutes(database));
  app.route("/v1/network", networkRoutes(network, limiter));
  app.route("/v1/devices", deviceRoutes(services));
  app.route("/v1/host-sign-ins", hostSignInRoutes(services));
  app.route("/v1/account/profile", profileRoutes(services));
  app.route("/v1/account", accountRoutes(services));
  app.route("/v1/webhooks", clerkWebhookRoutes(database, webhookSecret));
  app.route("/v1/hooks", hookRoutes(services));
  app.route("/updates", updateRoutes(database, limiter));

  app.notFound((c) =>
    errorResponse(
      c,
      404,
      "not_found",
      `No route matches ${c.req.method} ${c.req.path}.`,
    ),
  );
  app.onError((error, c) => {
    if (error instanceof ApiFailure)
      return errorResponse(
        c,
        error.status,
        error.code,
        error.message,
        error.extras,
      );
    c.get("log").error({ err: error }, "a request failed");
    return errorResponse(c, 500, "internal", "Something failed on the server.");
  });

  return app;
}
