import type { Network } from "@sikemux/protocol";
import { Hono } from "hono";
import { bodyLimit } from "hono/body-limit";
import { cors } from "hono/cors";

import type { AppleSignIn } from "./account/apple.ts";
import { profileRoutes } from "./account/profile.ts";
import type { ClerkBackend } from "./account/clerk.ts";
import { accountRoutes } from "./account/routes.ts";
import type { Verifier } from "./auth.ts";
import type { Database } from "./db.ts";
import { deviceRoutes } from "./devices/routes.ts";
import { healthRoutes } from "./health/routes.ts";
import { hostSignInRoutes } from "./host/routes.ts";
import { ApiFailure, errorResponse, requestContext, type Env } from "./http.ts";
import type { JoinSigner } from "./join/signer.ts";
import { clientAddress, limit, RateLimiter } from "./limits.ts";
import type { Logger } from "./log.ts";
import { networkRoutes } from "./network/routes.ts";
import type { Places } from "./places.ts";
import type { PushSettings } from "./push/settings.ts";
import { RelayAccess, relayAccessRoutes } from "./relay/access.ts";
import { updateRoutes } from "./updates/routes.ts";
import { clerkWebhookRoutes } from "./webhooks/clerk.ts";

export interface Services {
  database: Database;
  log: Logger;
  appOrigin: string;
  verifier: Verifier;
  limiter?: RateLimiter;
  relayAccess?: RelayAccess;
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
}

const MAX_BODY_BYTES = 64 * 1024;

export function createApp({
  database,
  log,
  appOrigin,
  verifier,
  limiter = new RateLimiter(),
  relayAccess = new RelayAccess(database.db),
  clerk,
  appleSignIn,
  webhookSecret,
  network,
  push,
  join,
  places,
  relayAccessToken,
}: Services) {
  const app = new Hono<Env>();

  app.use(requestContext(log));
  app.route("/internal", relayAccessRoutes(relayAccess, relayAccessToken));
  app.use(limit<Env>(limiter, "address", 600, clientAddress));
  app.use(
    cors({
      origin: appOrigin,
      allowMethods: ["GET", "POST", "PATCH", "DELETE"],
      allowHeaders: ["authorization", "content-type"],
      exposeHeaders: ["x-request-id"],
      maxAge: 600,
    }),
  );
  app.use(
    bodyLimit({
      maxSize: MAX_BODY_BYTES,
      onError: (c) =>
        errorResponse(
          c,
          413,
          "payload_too_large",
          `Request bodies are limited to ${MAX_BODY_BYTES} bytes.`,
        ),
    }),
  );

  app.route("/v1/health", healthRoutes(database));
  app.route("/v1/network", networkRoutes(network, limiter));
  app.route(
    "/v1/devices",
    deviceRoutes(database, verifier, limiter, clerk, push, join),
  );
  app.route(
    "/v1/host-sign-ins",
    hostSignInRoutes(database, verifier, limiter, appOrigin, places),
  );
  app.route(
    "/v1/account/profile",
    profileRoutes(database, verifier, limiter, clerk),
  );
  app.route(
    "/v1/account",
    accountRoutes(database, verifier, limiter, clerk, appleSignIn, push.app),
  );
  app.route("/v1/webhooks", clerkWebhookRoutes(database, webhookSecret));
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
