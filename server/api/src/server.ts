import { resolveTxt } from "node:dns/promises";
import { mkdirSync } from "node:fs";
import { serve } from "@hono/node-server";
import type { Server } from "node:http";
import type { Duplex } from "node:stream";

import { appleSignIn } from "./account/apple.ts";
import { clerkMacSignIn } from "./devices/mac-sign-in.ts";
import { workGoogle } from "./account/work-google.ts";
import { admittingVerifier } from "./account/admission.ts";
import { clerkBackend } from "./account/clerk.ts";
import { sweepClerk } from "./account/clerk-sweeper.ts";
import { makeDueCalls } from "./billing/calls.ts";
import { sweepDeletedBilling } from "./billing/deleted.ts";
import { alertWaitingKeptRefunds } from "./billing/seat-pause.ts";
import { PaddleProvider } from "./billing/paddle.ts";
import { PaddleApi } from "./billing/paddle-api.ts";
import { endSandboxPlans } from "./billing/sandbox.ts";
import { createApp } from "./app.ts";
import { clerkVerifier } from "./auth.ts";
import type { Config } from "./config.ts";
import { openDatabase } from "./db.ts";
import { RateLimiter } from "./limits.ts";
import { HookHosts } from "./hooks/hosts.ts";
import { attachHooks } from "./hooks/server.ts";
import { attachLive } from "./live/server.ts";
import { NO_PLACES, openPlaces } from "./places.ts";
import type { Logger } from "./log.ts";
import { ApnsProvider } from "./push/apns.ts";
import { FcmProvider } from "./push/fcm.ts";
import { Pusher } from "./push/send.ts";
import { RelayAccess } from "./relay/access.ts";
import { attachShares } from "./share/server.ts";
import { StashFiles } from "./stash/files.ts";
import { StandInProvider } from "./billing/stand-in.ts";

/** How long a stop waits for requests in flight before closing them anyway. */
const DRAIN_MS = 25_000;
/** How long a stop waits for sockets to answer its goodbye before dropping the ones that haven't. */
const GOODBYE_MS = 1_000;
const SWEEP_MS = 60_000;

export async function startServer(config: Config, log: Logger) {
  const database = openDatabase(config.databaseUrl, log);
  const signInVerifier = clerkVerifier({
    issuer: config.clerkIssuer,
    macClientId: config.macClientId,
    authorizedParties: [config.appOrigin],
  });
  const limiter = new RateLimiter();
  const clerk = config.clerkSecretKey
    ? clerkBackend(config.clerkSecretKey, fetch, config.clerkApi)
    : null;
  if (!clerk)
    log.warn(
      "CLERK_SECRET_KEY is not set: deleted accounts and removed phones' sessions wait to be deleted in Clerk",
    );
  const verifier = admittingVerifier(
    signInVerifier,
    database.db,
    clerk,
    config.addressProofKey,
    log,
  );
  const apple = config.appleSignIn ? appleSignIn(config.appleSignIn) : null;
  if (apple)
    log.info(
      { keyId: config.appleSignIn?.keyId },
      "revoking Sign in with Apple when accounts are deleted",
    );
  else
    log.warn(
      "APPLE_SIGNIN_KEY_FILE is not set: deleting an account cannot revoke Sign in with Apple",
    );
  if (!config.clerkWebhookSecret)
    log.warn("CLERK_WEBHOOK_SECRET is not set: Clerk's webhooks are refused");
  const fcm = config.push.fcm ? new FcmProvider(config.push.fcm) : null;
  if (fcm)
    log.info(
      { project: config.push.fcm?.projectId, app: config.push.app },
      "pushing to Android through FCM",
    );
  else
    log.warn(
      "FCM_SERVICE_ACCOUNT_FILE is not set: pushes to Android phones answer not_set_up",
    );
  const apns = config.push.apns
    ? new ApnsProvider(config.push.apns, { topic: config.push.apns.topic })
    : null;
  if (apns)
    log.info(
      { keyId: config.push.apns?.keyId, topic: config.push.apns?.topic },
      "pushing to iOS through APNs",
    );
  else
    log.warn("APNS_KEY_FILE is not set: pushes to iPhones answer not_set_up");
  log.info(
    {
      keyId: config.join.keyId,
      publicKey: config.join.publicKey,
      file: config.join.file,
      created: config.join.created,
    },
    "signing join tickets",
  );
  const pusher = new Pusher({
    db: database.db,
    log,
    limiter,
    providers: {
      ...(fcm ? { fcm } : {}),
      ...(apns ? { apns } : {}),
    },
  });
  const places = config.placesDir
    ? openPlaces(config.placesDir, log)
    : NO_PLACES;
  if (!config.placesDir)
    log.warn(
      "GEOIP_DIR is not set: a host asking to sign in shows no country or network",
    );
  if (!config.relayAccessToken)
    log.warn(
      "RELAY_ACCESS_TOKEN is not set: the relay is refused every device it asks about",
    );
  if (config.stash)
    mkdirSync(config.stash.dir, { recursive: true, mode: 0o700 });
  else
    log.warn(
      "STASH_DIR and STASH_KEY are not set: the stash answers unavailable",
    );
  const relayAccess = new RelayAccess(database.db);
  const hookHosts = new HookHosts();
  const billing = config.billing;
  const provider =
    billing.provider === "stand-in"
      ? new StandInProvider(config.apiOrigin)
      : billing.provider === "paddle"
        ? new PaddleProvider({
            api: new PaddleApi(billing.paddleKey, log),
            apiOrigin: config.apiOrigin,
            log,
          })
        : null;
  if (provider instanceof PaddleProvider)
    log.info(
      { sandbox: provider.sandbox },
      provider.sandbox
        ? "billing goes through Paddle's sandbox"
        : "billing goes through Paddle's live account",
    );
  if (provider instanceof PaddleProvider && !provider.sandbox)
    await endSandboxPlans(database.db, log);
  const app = createApp({
    database,
    log,
    appOrigin: config.appOrigin,
    apiOrigin: config.apiOrigin,
    verifier,
    signInVerifier,
    addressProofKey: config.addressProofKey,
    workGoogle: config.workGoogle ? workGoogle(config.workGoogle) : null,
    limiter,
    relayAccess,
    clerk,
    appleSignIn: apple,
    macSignIn: clerkMacSignIn(config.clerkIssuer, config.macClientId),
    webhookSecret: config.clerkWebhookSecret,
    network: config.network,
    push: config.push,
    join: config.join,
    places,
    relayAccessToken: config.relayAccessToken,
    stash: config.stash ? new StashFiles(config.stash) : null,
    hookHosts,
    txtRecords: async (name) =>
      (await resolveTxt(name)).map((chunks) => chunks.join("")),
    billing: { provider, live: config.billing.live },
  });
  const server = serve(
    { fetch: app.fetch, hostname: config.host, port: config.port },
    (address) =>
      log.info({ host: address.address, port: address.port }, "listening"),
  ) as Server;
  const shares = attachShares(server, {
    database,
    verifier,
    limiter,
    log,
    appOrigin: config.appOrigin,
  });

  const hooks = attachHooks(server, {
    database,
    verifier,
    limiter,
    log,
    hosts: hookHosts,
  });

  const live = attachLive(server, {
    database,
    databaseUrl: config.databaseUrl,
    verifier,
    limiter,
    log,
    appOrigin: config.appOrigin,
    pusher,
    watchers: [shares, relayAccess, hooks],
  });
  let sweeping = false;
  const sweep = setInterval(() => {
    if (sweeping) return;
    sweeping = true;
    Promise.all([
      clerk &&
        sweepClerk(database.db, clerk, log).catch((error: unknown) =>
          log.error({ err: error }, "retrying calls to Clerk failed"),
        ),
      provider &&
        makeDueCalls(database.db, provider, log).catch((error: unknown) =>
          log.error(
            { err: error },
            "making the calls billing owes the payment provider failed",
          ),
        ),
      alertWaitingKeptRefunds(database.db, log).catch((error: unknown) =>
        log.error({ err: error }, "checking waiting kept refunds failed"),
      ),
      sweepDeletedBilling(database.db, provider, log).catch((error: unknown) =>
        log.error(
          { err: error },
          "ending deleted accounts' billing with the payment provider failed",
        ),
      ),
    ]).finally(() => {
      sweeping = false;
    });
  }, SWEEP_MS);

  const upgraded = new Set<Duplex>();
  server.on("upgrade", (_request: unknown, socket: Duplex) => {
    upgraded.add(socket);
    socket.once("close", () => upgraded.delete(socket));
  });

  let stopping = false;
  const stop = (signal: NodeJS.Signals) => {
    if (stopping) return;
    stopping = true;
    log.info({ signal }, "stopping");
    clearInterval(sweep);
    const force = setTimeout(() => server.closeAllConnections(), DRAIN_MS);
    force.unref();
    const liveStopped = Promise.race([
      Promise.all([live.stop(), shares.stop(), hooks.stop()]),
      new Promise((resolve) => setTimeout(resolve, GOODBYE_MS)),
    ])
      .catch((error: unknown) =>
        log.error({ err: error }, "closing live connections failed"),
      )
      .then(() => {
        for (const socket of upgraded) socket.destroy();
      });
    server.close(() => {
      clearTimeout(force);
      liveStopped
        .then(() => database.close())
        .then(
          () => {
            log.info("stopped");
            process.exit(0);
          },
          (error: unknown) => {
            log.error({ err: error }, "closing the database failed");
            process.exit(1);
          },
        );
    });
    server.closeIdleConnections();
  };
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
}
