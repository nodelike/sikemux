import type { Network } from "@sikemux/protocol";
import { Hono } from "hono";
import { cors } from "hono/cors";

import type { AppleSignIn } from "./account/apple.ts";
import {
  macSignInRoutes,
  type MacSignInExchange,
} from "./devices/mac-sign-in.ts";
import { profileRoutes } from "./account/profile.ts";
import {
  reviewSignInRoutes,
  type ReviewAccount,
} from "./account/review-sign-in.ts";
import type { ClerkBackend } from "./account/clerk.ts";
import { foldRoutes } from "./account/fold.ts";
import { accountRoutes } from "./account/routes.ts";
import { workAddressRoutes } from "./account/work-addresses.ts";
import type { WorkGoogle } from "./account/work-google.ts";
import type { Verifier } from "./auth.ts";
import type { Database } from "./db.ts";
import { allowanceRoutes } from "./billing/host.ts";
import { billingProviderRoutes, billingRoutes } from "./billing/routes.ts";
import type { BillingProvider } from "./billing/provider.ts";
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
import type { Presence } from "./live/hub.ts";
import type { JoinSigner } from "./join/signer.ts";
import { clientAddress, limit, type RateLimiter } from "./limits.ts";
import type { Logger } from "./log.ts";
import { networkRoutes } from "./network/routes.ts";
import { membershipRoutes } from "./orgs/host.ts";
import { invitePreviewRoutes, orgRoutes } from "./orgs/routes.ts";
import { orgVaultRoutes } from "./orgvault/routes.ts";
import { shareRoutes } from "./orgs/shares.ts";
import type { Places } from "./places.ts";
import type { PushSettings } from "./push/settings.ts";
import { relayAccessRoutes, type RelayAccess } from "./relay/access.ts";
import type { StashFiles } from "./stash/files.ts";
import { stashRoutes } from "./stash/routes.ts";
import { updateRoutes } from "./updates/routes.ts";
import { vaultRoutes } from "./vault/routes.ts";
import { modRoutes } from "./mods/routes.ts";
import { clerkWebhookRoutes } from "./webhooks/clerk.ts";

export interface Services {
  database: Database;
  log: Logger;
  appOrigin: string;
  /** The API's own public origin, which webhook addresses start with. */
  apiOrigin: string;
  verifier: Verifier;
  /** Clerk's sign-ins before the sign-up check, for the routes that fold a refused account away. */
  signInVerifier: Verifier;
  /** The key proven work addresses are hashed with, or null when ADDRESS_PROOF_KEY is not set. */
  addressProofKey: Buffer | null;
  /** Google, for proving a work address; null when its client is not set up. */
  workGoogle: WorkGoogle | null;
  limiter: RateLimiter;
  relayAccess: RelayAccess;
  /** Clerk's Backend API, or null when CLERK_SECRET_KEY is not set. */
  clerk: ClerkBackend | null;
  /** The store reviewers' demo account, or null when REVIEW_ACCOUNT_USER_ID is not set. */
  review: ReviewAccount | null;
  /** Apple's token endpoints, or null when APPLE_SIGNIN_KEY_FILE is not set. */
  appleSignIn: AppleSignIn | null;
  /** Clerk's token endpoint, where a Mac's browser sign-in code is traded. */
  macSignIn: MacSignInExchange;
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
  /** Which devices are connected to /v1/live now. */
  presence: Presence;
  /** The TXT records at a DNS name, each one's strings joined; throws when the lookup fails. */
  txtRecords: (name: string) => Promise<string[]>;
  billing: {
    /** Null when BILLING_PROVIDER is not set, and buying or changing anything answers unavailable. */
    provider: BillingProvider | null;
    live: boolean;
  };
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
  app.route("/v1/billing/provider", billingProviderRoutes(services));
  app.use(limit<Env>(limiter, "address", 600, clientAddress));
  app.use(
    cors({
      origin: appOrigin,
      allowMethods: ["GET", "POST", "PUT", "PATCH", "DELETE"],
      // The web app sends Sentry's trace headers, so its requests and the API's spans join up.
      allowHeaders: [
        "authorization",
        "content-type",
        "sentry-trace",
        "baggage",
      ],
      exposeHeaders: ["x-request-id"],
      maxAge: 600,
    }),
  );
  // The stash takes files, so it limits its own bodies.
  app.route("/v1/stash", stashRoutes(services));
  // A vault rotation carries every entry sealed again, so the vault limits its own bodies too.
  app.route("/v1/vault", vaultRoutes(services));
  app.route("/v1/orgs/:orgId/vault", orgVaultRoutes(services));
  // A push carries many sealed documents and a mod's files are uploaded, so mods limit their own bodies too.
  app.route("/v1/mods", modRoutes(services));
  app.use(limitBody());

  app.route("/v1/health", healthRoutes(database));
  app.route("/v1/network", networkRoutes(network, limiter));
  app.route("/v1/devices", deviceRoutes(services));
  app.route("/v1/host-sign-ins", hostSignInRoutes(services));
  app.route("/v1/mac-sign-ins", macSignInRoutes(services));
  app.route("/v1/review-sign-in", reviewSignInRoutes(services));
  app.route("/v1/account/profile", profileRoutes(services));
  app.route("/v1/account/allowance", allowanceRoutes(services));
  app.route("/v1/account/orgs", membershipRoutes(services));
  const { shares, tickets } = shareRoutes(services);
  app.route("/v1/account/shares", shares);
  app.route("/v1/account/peer-tickets", tickets);
  app.route("/v1/account/work-addresses", workAddressRoutes(services));
  app.route("/v1/account", foldRoutes(services));
  app.route("/v1/account", accountRoutes(services));
  app.route("/v1/orgs/join/preview", invitePreviewRoutes(services));
  app.route("/v1/orgs", orgRoutes(services));
  app.route(
    "/v1/webhooks",
    clerkWebhookRoutes(
      database,
      webhookSecret,
      services.billing.provider,
      services.clerk,
      services.addressProofKey,
      services.review,
    ),
  );
  app.route("/v1/hooks", hookRoutes(services));
  app.route("/v1/billing", billingRoutes(services));
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
